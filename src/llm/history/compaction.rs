use std::iter;
use std::mem;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use super::AssistantEvent;
use super::CompactionMessage;
use super::DeveloperMessage;
use super::History;
use super::Message;
use super::Splice;
use super::UserMessage;
use super::archive::ArchivedHistory;
use super::archive::ArchivedHistoryReason;
use super::message::AssistantMessage;
use super::message::AssistantStatus;
use super::state::HistoryState;
use super::tokens::TokenCount;

const COMPACTION_PROMPT: &str = "Summarize this conversation for future continuation. Keep concrete user requirements, decisions, constraints, file paths, and unresolved work. Be concise and factual. Output plain text only.";

/// a summary of the first `n_drop` messages, to replace them
#[derive(Clone, Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct Compaction {
    pub n_drop: usize,
    pub summary: CompactionMessage,
}

/// folds a summary request's stream into the summary
pub struct Summary(HistoryState);

impl Summary {
    pub fn new(created_at: u64) -> Self {
        Self(vec![AssistantMessage::new(created_at).into()].into())
    }

    pub fn handle(
        &mut self,
        event: AssistantEvent,
    ) -> Result<()> {
        self.0.handle_response(event)
    }

    pub fn finish(self) -> Result<CompactionMessage> {
        let msg = self
            .0
            .last()
            .and_then(Message::try_as_assistant_ref)
            .context("no summary response")?;
        match &msg.status {
            AssistantStatus::Success => {}
            AssistantStatus::Error(e) => bail!("{e}"),
            AssistantStatus::Queued | AssistantStatus::InProgress => {
                bail!("summary response did not complete")
            }
        }
        let text = msg.text_output().trim().to_string();
        anyhow::ensure!(!text.is_empty(), "compaction summary is empty");
        Ok(CompactionMessage {
            text,
            token_count: 0,
            created_at: msg.created_at,
            started_at: msg.started_at.unwrap_or(msg.created_at),
            ended_at: msg.ended_at.context("summary response has no ended_at")?,
        })
    }
}

impl History {
    /// a summary request for the first `n_drop` messages
    pub fn compaction_input(
        &self,
        n_drop: usize,
        created_at: u64,
    ) -> Vec<Message> {
        let prompt = UserMessage::new(COMPACTION_PROMPT.into(), created_at);
        self.state.messages[..n_drop]
            .iter()
            .cloned()
            .chain(iter::once(Message::User(prompt)))
            .collect()
    }

    /// replace the summarized messages, archiving the full history
    pub fn compact(
        &mut self,
        Compaction { n_drop, summary }: Compaction,
    ) -> Result<Splice> {
        let tail = self
            .state
            .messages
            .get(n_drop..)
            .context("compaction past the end of history")?
            .to_vec();
        let mut summary = Message::Developer(DeveloperMessage::Compaction(summary));
        summary.recount();
        let state = iter::once(summary).chain(tail).collect::<Vec<_>>().into();
        self.archive.push(ArchivedHistory {
            state: mem::replace(&mut self.state, state),
            reason: ArchivedHistoryReason::Compaction,
        });
        Ok(Splice {
            range: 0..n_drop,
            len: 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::llm::history::HistoryUpdate;
    use crate::llm::history::message::AssistantItem;
    use crate::llm::history::message::OutputContent;
    use crate::llm::history::message::OutputItem;

    fn user(text: &str) -> HistoryUpdate {
        HistoryUpdate::UserMessage(UserMessage::new(text.into(), 0))
    }

    fn history(texts: &[&str]) -> History {
        let mut history = History::new(String::new());
        for text in texts {
            history.handle(user(text)).unwrap();
        }
        history
    }

    fn compaction(n_drop: usize) -> HistoryUpdate {
        HistoryUpdate::Compaction(Compaction {
            n_drop,
            summary: CompactionMessage {
                text: "summary".into(),
                token_count: 0,
                created_at: 1,
                started_at: 2,
                ended_at: 3,
            },
        })
    }

    fn output(text: &str) -> AssistantEvent {
        let mut item = OutputItem::new("out".into(), 0);
        item.content = vec![OutputContent::Text(text.into())];
        AssistantEvent::Item(Box::new(AssistantItem::Output(item)))
    }

    fn summarize(events: Vec<AssistantEvent>) -> Result<CompactionMessage> {
        let mut summary = Summary::new(1);
        for event in events {
            summary.handle(event)?;
        }
        summary.finish()
    }

    #[test]
    fn compaction_replaces_prefix_and_archives_history() {
        let mut history = history(&["first", "second", "last"]);

        let splices = history.handle(compaction(2)).unwrap();

        assert_eq!(
            splices,
            [Splice {
                range: 0..2,
                len: 1
            }]
        );

        insta::assert_yaml_snapshot!(history.state().messages, @r#"
        - role: developer
          Compaction:
            text: summary
            token_count: 1
            created_at: 1
            started_at: 2
            ended_at: 3
        - role: user
          text: last
          token_count: 1
          created_at: 0
        "#);
        assert_eq!(history.archive.len(), 1);
        assert_eq!(history.archive[0].state.messages.len(), 3);
        assert_eq!(
            history.state().token_count(),
            history
                .state()
                .iter()
                .map(TokenCount::token_count)
                .sum::<usize>()
        );
    }

    #[test]
    fn compaction_past_history_end_is_rejected() {
        let mut history = history(&["only"]);

        assert!(history.handle(compaction(2)).is_err());

        assert_eq!(history.state().messages.len(), 1);
        assert!(history.archive.is_empty());
    }

    #[test]
    fn compaction_input_appends_prompt_to_prefix() {
        let history = history(&["first", "second"]);

        insta::assert_yaml_snapshot!(history.compaction_input(1, 5), @r#"
        - role: user
          text: first
          token_count: 1
          created_at: 0
        - role: user
          text: "Summarize this conversation for future continuation. Keep concrete user requirements, decisions, constraints, file paths, and unresolved work. Be concise and factual. Output plain text only."
          token_count: 35
          created_at: 5
        "#);
    }

    #[test]
    fn summary_is_the_trimmed_text_output() {
        let summary = summarize(vec![
            AssistantEvent::Started { started_at: 2 },
            output("  the gist \n"),
            AssistantEvent::Completed { ended_at: 3 },
        ])
        .unwrap();

        insta::assert_yaml_snapshot!(summary, @"
        text: the gist
        token_count: 0
        created_at: 1
        started_at: 2
        ended_at: 3
        ");
    }

    #[test]
    fn failed_summary_is_an_error() {
        let result = summarize(vec![
            output("partial"),
            AssistantEvent::Failed {
                message: "rate limited".into(),
                ended_at: 3,
            },
        ]);

        assert_eq!(result.unwrap_err().to_string(), "rate limited");
    }

    #[test]
    fn empty_summary_is_an_error() {
        let result = summarize(vec![AssistantEvent::Completed { ended_at: 3 }]);

        assert_eq!(result.unwrap_err().to_string(), "compaction summary is empty");
    }

    #[test]
    fn unfinished_summary_is_an_error() {
        let result = summarize(vec![output("partial")]);

        assert_eq!(
            result.unwrap_err().to_string(),
            "summary response did not complete"
        );
    }
}

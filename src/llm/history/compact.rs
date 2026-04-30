use std::iter;
use std::mem;

use anyhow::Result;
use anyhow::bail;
use derive_more::Deref;
use derive_more::DerefMut;
use serde::Deserialize;
use serde::Serialize;

use super::CompactMessage;
use super::DeveloperMessage;
use super::History;
use super::Message;
use super::UserMessage;
use super::archive::ArchivedHistory;
use super::archive::ArchivedHistoryReason;
use super::state::HistoryState;
use super::timing::now;
use super::tokens::TokenCount;

const COMPACT_PROMPT: &str = "Summarize this conversation for future continuation. Keep concrete user requirements, decisions, constraints, file paths, and unresolved work. Be concise and factual. Output plain text only.";

#[derive(Default, Clone, Serialize, Deserialize, Debug, Deref, DerefMut)]
pub struct CompactState {
    #[deref]
    #[deref_mut]
    pub state: HistoryState,
    pub n_drop: usize,
    pub needs_another_turn: bool,

    pub created_at: u64,
    pub started_at: Option<u64>,
}

// TODO rename
#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Activity {
    Normal {
        state: HistoryState,
    },
    Compacting {
        state: HistoryState,
        compact: CompactState,
    },
}

impl Default for Activity {
    fn default() -> Self {
        Self::Normal {
            state: HistoryState::default(),
        }
    }
}

impl History {
    pub fn init_compact(
        &mut self,
        n_drop: usize,
    ) -> Result<()> {
        let Activity::Normal { state } = mem::take(&mut self.activity) else {
            bail!("compact already in progress");
        };
        let needs_another_turn = state.needs_another_turn();
        let compact_messages: Vec<_> = state
            .messages
            .iter()
            .take(n_drop)
            .cloned()
            .chain(iter::once(Message::User(UserMessage::new(
                COMPACT_PROMPT.into(),
            ))))
            .collect();
        let compact = CompactState {
            state: HistoryState::from(compact_messages),
            n_drop,
            needs_another_turn,
            created_at: now(),
            started_at: None,
        };
        self.activity = Activity::Compacting { state, compact };
        Ok(())
    }

    pub fn compact_turn_input(&self) -> Result<Vec<Message>> {
        match &self.activity {
            Activity::Compacting { compact, .. } => Ok(compact.state.messages.clone()),
            Activity::Normal { .. } => bail!("no compact available"),
        }
    }

    pub const fn compacting(&self) -> bool {
        matches!(self.activity, Activity::Compacting { .. })
    }

    pub fn apply_compact(&mut self) -> Result<()> {
        let (old_state, compact) = {
            let Activity::Compacting { state, compact } = &mut self.activity else {
                bail!("no compact in progress");
            };
            (state.clone(), compact)
        };
        let new_state = build_compacted(&old_state, compact)?;

        self.activity = Activity::Normal { state: new_state };
        self.archive.push(ArchivedHistory {
            state: old_state,
            reason: ArchivedHistoryReason::Compact,
        });
        Ok(())
    }
}

fn build_compacted(
    old_state: &HistoryState,
    compact: &CompactState,
) -> Result<HistoryState> {
    let summary = compact
        .state
        .text_outputs_after(compact.n_drop)
        .trim()
        .to_string();

    {
        if summary.is_empty() {
            bail!("compact summary is empty");
        }
        let len = old_state.messages.len();
        if compact.n_drop > len {
            bail!(
                "cannot compact first {} messages from history of length {len}",
                compact.n_drop
            );
        }
    }

    // TODO emit warning when unwrapping?
    let started_at = compact.started_at.unwrap_or(compact.created_at);
    let ended_at = compact
        .state
        .last()
        .and_then(super::timing::Timing::ended_at)
        .unwrap_or_else(now);

    let compact_msg = {
        let mut msg: Message = DeveloperMessage::Compact(CompactMessage {
            text: summary,
            needs_another_turn: compact.needs_another_turn,
            created_at: compact.created_at,
            started_at,
            ready_at: ended_at,
            token_count: 0,
        })
        .into();
        msg.recount();
        msg
    };

    Ok(iter::once(compact_msg)
        .chain(old_state.messages.iter().skip(compact.n_drop).cloned())
        .collect::<Vec<_>>()
        .into())
}

#[cfg(test)]
mod tests {
    use indexmap::indexmap;
    use similar_asserts::assert_eq;

    use super::*;
    use crate::llm::history::AssistantEvent;
    use crate::llm::history::History;
    use crate::llm::history::HistoryUpdate;
    use crate::llm::history::message::AssistantItem;
    use crate::llm::history::message::AssistantMessage;
    use crate::llm::history::message::AssistantStatus;
    use crate::llm::history::message::OutputContent;
    use crate::llm::history::message::OutputItem;
    use crate::llm::history::message::UserMessage;
    use crate::llm::history::tokens::TokenCount;

    fn compact_response(event: AssistantEvent) -> HistoryUpdate {
        HistoryUpdate::CompactResponse(event)
    }

    fn output_item(
        id: &str,
        text: Option<&str>,
    ) -> AssistantItem {
        let mut item = OutputItem::new(id.into());
        if let Some(text) = text {
            item.content = vec![OutputContent::Text(text.into())];
        }
        AssistantItem::Output(item)
    }

    fn compact_summary(text: &str) -> Message {
        let mut msg = AssistantMessage::new(0);
        msg.status = AssistantStatus::Success;
        msg.content = indexmap! {
            "out".into() => output_item("out", Some(text)),
        };
        Message::Assistant(msg)
    }

    fn push(
        history: &mut History,
        msg: Message,
    ) {
        match &mut history.activity {
            Activity::Normal { state } | Activity::Compacting { state, .. } => state.push(msg),
        }
    }

    #[test]
    fn compact_completed_applies_and_archives_history() {
        let mut history = History::new(String::new());
        push(
            &mut history,
            Message::User(UserMessage::new("first".into())),
        );
        push(&mut history, compact_summary("old reply"));
        push(&mut history, Message::User(UserMessage::new("last".into())));
        if let Activity::Normal { state } = &mut history.activity {
            state.recount();
        } else {
            panic!("expected normal turn");
        }
        let generation = history.generation();

        history
            .handle(generation, HistoryUpdate::CompactStart { n_drop: 2 })
            .unwrap();
        history
            .handle(generation, compact_response(AssistantEvent::Created(0)))
            .unwrap();
        history
            .handle(
                generation,
                compact_response(AssistantEvent::Item(Box::new(output_item(
                    "out",
                    Some("summary"),
                )))),
            )
            .unwrap();
        history
            .handle(
                generation,
                compact_response(AssistantEvent::Completed(vec![output_item("out", None)])),
            )
            .unwrap();

        assert!(!history.compacting());
        assert_eq!(history.archive.len(), 1);
        assert_eq!(history.state().messages.len(), 2);
        assert!(matches!(
            &history.state().messages[0],
            Message::Developer(DeveloperMessage::Compact(CompactMessage { text, .. })) if text == "summary"
        ));
        assert!(
            matches!(&history.state().messages[1], Message::User(UserMessage { text, .. }) if text == "last")
        );
    }

    #[test]
    fn compact_failed_keeps_state_without_rewriting_history() {
        let mut history = History::new(String::new());
        push(
            &mut history,
            Message::User(UserMessage::new("first".into())),
        );
        push(&mut history, compact_summary("reply"));
        if let Activity::Normal { state } = &mut history.activity {
            state.recount();
        } else {
            panic!("expected normal turn");
        }
        let generation = history.generation();
        let total_tokens = history.state().token_count();

        history
            .handle(generation, HistoryUpdate::CompactStart { n_drop: 1 })
            .unwrap();
        history
            .handle(
                generation,
                compact_response(AssistantEvent::Failed("oops".into())),
            )
            .unwrap();

        assert!(history.compacting());
        assert_eq!(history.archive.len(), 0);
        assert_eq!(history.state().messages.len(), 2);
        assert_eq!(history.state().token_count(), total_tokens);
        assert!(
            matches!(&history.state().messages[0], Message::User(UserMessage { text, .. }) if text == "first")
        );
        assert!(matches!(
            &history.state().messages[1],
            Message::Assistant(AssistantMessage {
                status: AssistantStatus::Success,
                ..
            })
        ));
    }

    #[test]
    fn compact_completed_concatenates_outputs_from_all_attempts() {
        let mut history = History::new(String::new());
        push(
            &mut history,
            Message::User(UserMessage::new("first".into())),
        );
        push(&mut history, compact_summary("reply"));
        if let Activity::Normal { state } = &mut history.activity {
            state.recount();
        } else {
            panic!("expected normal turn");
        }
        let generation = history.generation();

        history
            .handle(generation, HistoryUpdate::CompactStart { n_drop: 1 })
            .unwrap();
        history
            .handle(generation, compact_response(AssistantEvent::Created(0)))
            .unwrap();
        history
            .handle(
                generation,
                compact_response(AssistantEvent::Item(Box::new(output_item(
                    "out-1",
                    Some("part 1"),
                )))),
            )
            .unwrap();
        history
            .handle(
                generation,
                compact_response(AssistantEvent::Failed("oops".into())),
            )
            .unwrap();
        history
            .handle(generation, compact_response(AssistantEvent::Created(0)))
            .unwrap();
        history
            .handle(
                generation,
                compact_response(AssistantEvent::Item(Box::new(output_item(
                    "out-2",
                    Some("part 2"),
                )))),
            )
            .unwrap();
        history
            .handle(
                generation,
                compact_response(AssistantEvent::Completed(vec![output_item("out-2", None)])),
            )
            .unwrap();

        assert!(matches!(
            &history.state().messages[0],
            Message::Developer(DeveloperMessage::Compact(CompactMessage { text, .. })) if text == "part 1part 2"
        ));
    }

    #[test]
    fn compact_completed_with_empty_summary_keeps_history() {
        let mut history = History::new(String::new());
        push(
            &mut history,
            Message::User(UserMessage::new("first".into())),
        );
        push(&mut history, compact_summary("reply"));
        if let Activity::Normal { state } = &mut history.activity {
            state.recount();
        } else {
            panic!("expected normal turn");
        }
        let generation = history.generation();
        let total_tokens = history.state().token_count();

        history
            .handle(generation, HistoryUpdate::CompactStart { n_drop: 1 })
            .unwrap();
        history
            .handle(generation, compact_response(AssistantEvent::Created(0)))
            .unwrap();

        let err = history
            .handle(
                generation,
                compact_response(AssistantEvent::Completed(vec![])),
            )
            .unwrap_err();

        assert_eq!(err.to_string(), "compact summary is empty");
        assert!(!history.compacting());
        assert_eq!(history.archive.len(), 0);
        assert_eq!(history.state().messages.len(), 2);
        assert_eq!(history.state().token_count(), total_tokens);
        assert!(
            matches!(&history.state().messages[0], Message::User(UserMessage { text, .. }) if text == "first")
        );
        assert!(matches!(
            &history.state().messages[1],
            Message::Assistant(AssistantMessage {
                status: AssistantStatus::Success,
                ..
            })
        ));
    }
}

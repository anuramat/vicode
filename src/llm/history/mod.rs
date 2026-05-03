// TODO unpub
mod archive;
mod compact;
pub mod delta;
mod event;
mod instructions;
pub mod message;
mod response;
mod state;
mod subagent;
mod timing;
mod tokens;

use anyhow::Result;
use anyhow::bail;
use archive::ArchivedHistory;
use archive::ArchivedHistoryReason;
pub use compact::Activity;
use compact::CompactState;
pub use event::AssistantEvent;
pub use event::HistoryGeneration;
pub use event::HistoryUpdate;
use instructions::Instructions;
pub use message::*;
use serde::Deserialize;
use serde::Serialize;
use state::HistoryState;
pub use state::TurnStatus;
pub use timing::Timing;
pub use tokens::TokenCount;
pub use tokens::count_text_tokens;
use tracing::instrument;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct History {
    instructions: Instructions,
    #[serde(skip)]
    generation: HistoryGeneration,
    activity: Activity,
    #[serde(default)]
    archive: Vec<ArchivedHistory>,
}

impl History {
    pub fn new(instructions: String) -> Self {
        Self {
            instructions: Instructions::new(instructions),
            generation: 0,
            activity: Activity::default(),
            archive: Vec::new(),
        }
    }

    pub const fn state(&self) -> &HistoryState {
        match &self.activity {
            Activity::Normal { state } | Activity::Compacting { state, .. } => state,
        }
    }

    pub const fn activity(&self) -> &Activity {
        &self.activity
    }

    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    // TODO move this to historystate or something?
    /// calculate how many messages we need to drop to get under the given percentage of the context window
    pub fn window_percentage_to_n_msg(
        &self,
        window: usize,
        target_percentage: usize,
    ) -> usize {
        let target = window.saturating_mul(target_percentage) / 100;
        let mut kept = self.token_count();
        for (idx, msg) in self.state().iter().enumerate() {
            if kept < target {
                return idx;
            }
            kept -= msg.token_count();
        }
        self.state().messages.len()
    }

    pub fn token_count(&self) -> usize {
        self.instructions.token_count() + self.state().token_count()
    }

    const fn increment(&mut self) {
        self.generation += 1;
    }

    pub const fn generation(&self) -> HistoryGeneration {
        self.generation
    }

    #[instrument(skip(self))]
    pub fn handle(
        &mut self,
        generation: HistoryGeneration,
        event: HistoryUpdate,
    ) -> Result<()> {
        anyhow::ensure!(
            generation == self.generation,
            "history generation mismatch: expected {}",
            self.generation,
        );
        match event {
            HistoryUpdate::CompactAbort => {
                self.abort_compact()?;
            }
            HistoryUpdate::GenerationIncremented => self.increment(),
            HistoryUpdate::DeveloperMessage(msg) => {
                self.normal_mut()?.push(Message::Developer(msg));
            }
            HistoryUpdate::UserMessage(text) => {
                self.normal_mut()?
                    .push(Message::User(UserMessage::new(text)));
            }
            HistoryUpdate::Pop(n) => {
                // TODO this condition should be computed smarter I think;
                // don't care about false positives, but as it is now, we might skip archiving when we should;
                // e.g. if we pop, then set history to an older version from the archive, and then pop again;
                // not a problem for now, since we don't have a way to set history to an older version.
                let should_archive = self
                    .archive
                    .last()
                    .is_none_or(|v| !matches!(v.reason, ArchivedHistoryReason::Undo));

                let state = self.normal_mut()?;
                let len = state.messages.len();

                let keep = len.saturating_sub(n);
                if keep == len {
                    return Ok(());
                }

                let archived = if should_archive {
                    Some(ArchivedHistory {
                        state: state.clone(),
                        reason: ArchivedHistoryReason::Undo,
                    })
                } else {
                    None
                };

                state.messages.truncate(keep);
                state.recount_shallow();
                if let Some(archived) = archived {
                    self.archive.push(archived);
                }
            }
            HistoryUpdate::TurnResponse(event) => {
                self.normal_mut()?.handle_response(event)?;
            }
            HistoryUpdate::CompactStart { n_drop } => self.init_compact(n_drop)?,
            HistoryUpdate::CompactResponse(event) => {
                let completed = matches!(event, AssistantEvent::Completed(_));
                self.compact_mut()?.state.handle_response(event)?;
                if completed && self.apply_compact().is_err() {
                    self.abort_compact()?;
                }
            }
        }
        Ok(())
    }

    fn normal_mut(&mut self) -> Result<&mut HistoryState> {
        match &mut self.activity {
            Activity::Normal { state } => Ok(state),
            Activity::Compacting { .. } => bail!("requires Normal state"),
        }
    }

    fn compact_mut(&mut self) -> Result<&mut CompactState> {
        match &mut self.activity {
            Activity::Compacting { compact, .. } => Ok(compact),
            Activity::Normal { .. } => bail!("requires Compacting state"),
        }
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::llm::history::delta::Delta;
    use crate::llm::history::message::AssistantItem;
    use crate::llm::history::message::DeveloperMessage;
    use crate::llm::history::message::OutputItem;

    fn response(event: AssistantEvent) -> HistoryUpdate {
        HistoryUpdate::TurnResponse(event)
    }

    #[test]
    fn generation_changes_only_when_message_count_changes() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created(1)))
            .unwrap();
        assert_eq!(history.generation(), 0);
        history
            .handle(
                0,
                response(AssistantEvent::Item(Box::new(AssistantItem::Output(
                    OutputItem {
                        id: "out".into(),
                        started_at: 2,
                        ended_at: None,
                        token_count: 0,
                        content: vec![],
                    },
                )))),
            )
            .unwrap();
        assert_eq!(history.generation(), 0);
        history
            .handle(
                0,
                response(AssistantEvent::Delta(Delta {
                    id: "out".into(),
                    delta: crate::llm::history::delta::DeltaContent::Output("hello".into()),
                })),
            )
            .unwrap();
        assert_eq!(history.generation(), 0);
        history
            .handle(0, HistoryUpdate::UserMessage("hi".into()))
            .unwrap();
        assert_eq!(history.generation(), 0);
        history.increment();
        assert_eq!(history.generation(), 1);
    }

    #[test]
    fn stale_generation_is_rejected() {
        let mut history = History::new(String::new());
        history
            .handle(0, HistoryUpdate::UserMessage("hello".into()))
            .unwrap();
        history.increment();
        assert!(history.handle(0, HistoryUpdate::Pop(1)).is_err());
        assert_eq!(history.state().messages.len(), 1);
    }

    #[test]
    fn pop_archives_dropped_tail() {
        let mut history = History::new(String::new());
        history
            .handle(0, HistoryUpdate::UserMessage("first".into()))
            .unwrap();
        history
            .handle(0, HistoryUpdate::UserMessage("second".into()))
            .unwrap();
        history.handle(0, HistoryUpdate::Pop(1)).unwrap();

        assert_eq!(history.state().messages.len(), 1);
        assert_eq!(history.archive.len(), 1);
        assert_eq!(history.archive[0].state.messages.len(), 1);
        assert!(matches!(
            &history.archive[0].state.messages[0],
            Message::User(UserMessage { text, .. }) if text == "second"
        ));
    }

    #[test]
    fn pop_zero_does_not_archive() {
        let mut history = History::new(String::new());
        history
            .handle(0, HistoryUpdate::UserMessage("only".into()))
            .unwrap();
        history.handle(0, HistoryUpdate::Pop(0)).unwrap();
        assert!(history.archive.is_empty());
    }

    #[test]
    fn generation_increment_event_updates_generation() {
        let mut history = History::new(String::new());
        history
            .handle(0, HistoryUpdate::GenerationIncremented)
            .unwrap();
        assert_eq!(history.generation(), 1);
        assert_eq!(history.state().messages.len(), 0);
    }

    #[test]
    fn response_can_follow_generation_increment() {
        let mut history = History::new(String::new());
        history
            .handle(0, HistoryUpdate::UserMessage("hello".into()))
            .unwrap();
        history
            .handle(0, HistoryUpdate::GenerationIncremented)
            .unwrap();
        history
            .handle(1, response(AssistantEvent::Created(7)))
            .unwrap();
        assert_eq!(history.generation(), 1);
        assert_eq!(history.state().messages.len(), 2);
    }

    #[test]
    fn generation_changes_for_external_history_edits() {
        let mut history = History::new(String::new());
        history
            .handle(
                0,
                HistoryUpdate::DeveloperMessage(DeveloperMessage::misc("note".into())),
            )
            .unwrap();
        assert_eq!(history.generation(), 0);
        history.increment();
        assert_eq!(history.generation(), 1);
        history.handle(1, HistoryUpdate::Pop(1)).unwrap();
        assert_eq!(history.generation(), 1);
    }
}

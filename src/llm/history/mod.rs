// TODO unpub
mod archive;
mod compaction;
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
use archive::ArchivedHistory;
use archive::ArchivedHistoryReason;
pub use compaction::Compaction;
pub use compaction::Summary;
pub use event::AssistantEvent;
pub use event::HistoryUpdate;
pub use event::Splice;
use instructions::Instructions;
pub use message::*;
use serde::Deserialize;
use serde::Serialize;
use state::HistoryState;
pub use timing::Timing;
pub use tokens::TokenCount;
pub use tokens::count_text_tokens;
use tracing::instrument;

use crate::agent::tool::registry::TOOL_REGISTRY;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct History {
    instructions: Instructions,
    state: HistoryState,
    #[serde(default)]
    archive: Vec<ArchivedHistory>,
}

impl History {
    pub fn new(instructions: String) -> Self {
        Self {
            instructions: Instructions::new(instructions),
            state: HistoryState::default(),
            archive: Vec::new(),
        }
    }

    pub fn state(&self) -> &HistoryState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut HistoryState {
        &mut self.state
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
        self.instructions.token_count() + self.state().token_count() + TOOL_REGISTRY.token_count()
    }

    /// fail whatever was left in flight: the last response and every
    /// unresolved tool call
    pub fn fail_unresolved(
        &mut self,
        msg: &str,
        ended_at: u64,
    ) -> Vec<Splice> {
        let mut touched = self.state.fail_tool_calls(None, msg);
        if self.state.fail_response(msg.into(), ended_at) {
            let last = self.state.messages.len() - 1;
            if touched.first() != Some(&last) {
                touched.insert(0, last);
            }
        }
        touched.into_iter().map(Splice::edit).collect()
    }

    /// apply `event`; returns exactly the messages it changed, as disjoint
    /// splices in descending order -- each applies as is after the ones
    /// before it
    #[instrument(skip(self))]
    pub fn handle(
        &mut self,
        event: HistoryUpdate,
    ) -> Result<Vec<Splice>> {
        let len = self.state.messages.len();
        let splices = match event {
            HistoryUpdate::ToolCallFailed { call_id, error } => self
                .state
                .fail_tool_calls(Some(&call_id), &error)
                .into_iter()
                .map(Splice::edit)
                .collect(),
            HistoryUpdate::ToolOutput { call_id, chunk } => {
                vec![Splice::edit(self.state.push_tool_output(&call_id, &chunk)?)]
            }
            HistoryUpdate::FailUnresolved { message, ended_at } => {
                self.fail_unresolved(&message, ended_at)
            }
            HistoryUpdate::DeveloperMessage(msg) => {
                self.state.push(Message::Developer(msg));
                vec![Splice::append(len)]
            }
            HistoryUpdate::UserMessage(msg) => {
                self.state.push(Message::User(msg));
                vec![Splice::append(len)]
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

                let state = &mut self.state;
                let keep = len.saturating_sub(n);
                if keep == len {
                    return Ok(Vec::new());
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
                vec![Splice {
                    range: keep..len,
                    len: 0,
                }]
            }
            // a new response, or an edit of the last one
            HistoryUpdate::TurnResponse(event) => {
                self.state.handle_response(event)?;
                if self.state.messages.len() > len {
                    vec![Splice::append(len)]
                } else {
                    len.checked_sub(1).map(Splice::edit).into_iter().collect()
                }
            }
            HistoryUpdate::Compaction(compaction) => vec![self.compact(compaction)?],
        };
        Ok(splices)
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::llm::history::delta::Delta;
    use crate::tools::bash::BashArguments;
    use crate::tools::bash::BashCall;
    use crate::tools::bash::BashResult;

    fn response(event: AssistantEvent) -> HistoryUpdate {
        HistoryUpdate::TurnResponse(event)
    }

    fn user(text: &str) -> HistoryUpdate {
        HistoryUpdate::UserMessage(UserMessage::new(text.into(), 0))
    }

    fn user_at(
        text: &str,
        created_at: u64,
    ) -> HistoryUpdate {
        HistoryUpdate::UserMessage(UserMessage::new(text.into(), created_at))
    }

    #[test]
    fn pop_snapshots_pre_pop_state_for_undo() {
        let mut history = History::new(String::new());
        history.handle(user("first")).unwrap();
        history.handle(user("second")).unwrap();
        history.handle(HistoryUpdate::Pop(1)).unwrap();

        assert_eq!(history.state().messages.len(), 1);
        assert_eq!(history.archive.len(), 1);
        assert_eq!(history.archive[0].state.messages.len(), 2);
        assert!(matches!(
            &history.archive[0].state.messages[0],
            Message::User(UserMessage { text, .. }) if text == "first"
        ));
        assert!(matches!(
            &history.archive[0].state.messages[1],
            Message::User(UserMessage { text, .. }) if text == "second"
        ));
    }

    #[test]
    fn pop_zero_does_not_archive() {
        let mut history = History::new(String::new());
        history.handle(user("only")).unwrap();
        history.handle(HistoryUpdate::Pop(0)).unwrap();
        assert!(history.archive.is_empty());
    }

    fn bash_call(
        call_id: &str,
        output: Option<Result<BashResult, String>>,
    ) -> HistoryUpdate {
        response(AssistantEvent::Item(Box::new(AssistantItem::ToolCall(
            ToolCallItem {
                id: Some(call_id.into()),
                call_id: call_id.into(),
                task: Box::new(BashCall {
                    arguments: Some(BashArguments {
                        command: "make".into(),
                    }),
                    meta: None,
                    output,
                }),
                streamed: String::new(),
                token_count: 0,
                started_at: 0,
                ended_at: None,
                ready_at: None,
            },
        ))))
    }

    /// every update reports exactly the messages it changed: failing what's
    /// in flight leaves the resolved call and the queued message alone
    #[test]
    fn updates_report_their_splices() {
        let mut history = History::new(String::new());
        let resolved = BashResult {
            output: "ok".into(),
            exit_status: Some(0),
            signal: None,
        };
        let events = [
            ("user", user("first")),
            ("created", response(AssistantEvent::Created { created_at: 1 })),
            ("started", response(AssistantEvent::Started { started_at: 2 })),
            ("resolved call", bash_call("done", Some(Ok(resolved)))),
            ("completed", response(AssistantEvent::Completed { ended_at: 3 })),
            ("created", response(AssistantEvent::Created { created_at: 4 })),
            ("running call", bash_call("running", None)),
            (
                "tool output",
                HistoryUpdate::ToolOutput {
                    call_id: "running".into(),
                    chunk: "partial".into(),
                },
            ),
            ("queued user", user("queued")),
            ("created", response(AssistantEvent::Created { created_at: 5 })),
            (
                "fail unresolved",
                HistoryUpdate::FailUnresolved {
                    message: "gone".into(),
                    ended_at: 6,
                },
            ),
            (
                "fail again",
                HistoryUpdate::FailUnresolved {
                    message: "gone".into(),
                    ended_at: 7,
                },
            ),
            ("pop", HistoryUpdate::Pop(2)),
            ("pop none", HistoryUpdate::Pop(0)),
        ];
        let report = events
            .into_iter()
            .map(|(label, event)| {
                let splices = history.handle(event).unwrap();
                let splices = splices
                    .iter()
                    .map(|s| format!("{}..{} -> {}", s.range.start, s.range.end, s.len))
                    .collect::<Vec<_>>();
                format!("{label}: [{}]", splices.join(", "))
            })
            .collect::<Vec<_>>()
            .join("\n");

        insta::assert_snapshot!(report, @"
        user: [0..0 -> 1]
        created: [1..1 -> 1]
        started: [1..2 -> 1]
        resolved call: [1..2 -> 1]
        completed: [1..2 -> 1]
        created: [2..2 -> 1]
        running call: [2..3 -> 1]
        tool output: [2..3 -> 1]
        queued user: [3..3 -> 1]
        created: [4..4 -> 1]
        fail unresolved: [4..5 -> 1, 2..3 -> 1]
        fail again: []
        pop: [3..5 -> 0]
        pop none: []
        ");
    }

    #[test]
    fn history_token_count_includes_tool_schemas() {
        let history = History::new(String::new());

        assert_eq!(history.token_count(), TOOL_REGISTRY.token_count());
    }

    #[test]
    fn replayed_history_updates_are_serialized_identically() {
        let events = vec![
            user_at("hi", 1),
            response(AssistantEvent::Created { created_at: 2 }),
            response(AssistantEvent::Started { started_at: 3 }),
            response(AssistantEvent::Delta(Delta {
                id: "out".into(),
                delta: crate::llm::history::delta::DeltaContent::Output("hello".into()),
                timestamp: 4,
            })),
            response(AssistantEvent::Completed { ended_at: 5 }),
        ];
        let mut left = History::new("instructions".into());
        let mut right = History::new("instructions".into());
        for event in events {
            left.handle(event.clone()).unwrap();
            right.handle(event).unwrap();
        }

        assert_eq!(
            serde_json::to_value(&left).unwrap(),
            serde_json::to_value(&right).unwrap()
        );
    }
}

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

    /// fail whatever a dead process left in flight, wherever it is
    pub fn fail_all_unresolved(
        &mut self,
        msg: &str,
        ended_at: u64,
    ) {
        self.state.fail_all_unresolved(msg, ended_at);
    }

    /// apply `event`; returns exactly the messages it changed, if any
    #[instrument(skip(self))]
    pub fn handle(
        &mut self,
        event: HistoryUpdate,
    ) -> Result<Option<Splice>> {
        let len = self.state.messages.len();
        let splice = match event {
            HistoryUpdate::ToolCallFailed { call_id, error } => self
                .state
                .fail_tool_call(&call_id, &error)?
                .then(|| Splice::edit(len - 1)),
            HistoryUpdate::ToolOutput { call_id, chunk } => {
                self.state.push_tool_output(&call_id, &chunk)?;
                Some(Splice::edit(len - 1))
            }
            HistoryUpdate::FailUnresolved { message, ended_at } => self
                .state
                .fail_last_unresolved(&message, ended_at)
                .then(|| Splice::edit(len - 1)),
            HistoryUpdate::DeveloperMessage(msg) => {
                self.state.push(Message::Developer(msg));
                Some(Splice::append(len))
            }
            HistoryUpdate::UserMessage(msg) => {
                self.state.push(Message::User(msg));
                Some(Splice::append(len))
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
                    return Ok(None);
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
                Some(Splice {
                    range: keep..len,
                    len: 0,
                })
            }
            // a new response, or an edit of the last one
            HistoryUpdate::TurnResponse(event) => {
                self.state.handle_response(event)?;
                if self.state.messages.len() > len {
                    Some(Splice::append(len))
                } else {
                    len.checked_sub(1).map(Splice::edit)
                }
            }
            HistoryUpdate::Compaction(compaction) => Some(self.compact(compaction)?),
        };
        Ok(splice)
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::agent::tool::generic::ToolOutcome;
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
        output: Option<ToolOutcome<BashResult>>,
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
    /// in flight leaves the resolved call alone
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
            (
                "created",
                response(AssistantEvent::Created { created_at: 1 }),
            ),
            (
                "started",
                response(AssistantEvent::Started { started_at: 2 }),
            ),
            (
                "resolved call",
                bash_call("done", Some(ToolOutcome::ok(resolved))),
            ),
            (
                "completed",
                response(AssistantEvent::Completed { ended_at: 3 }),
            ),
            (
                "created",
                response(AssistantEvent::Created { created_at: 4 }),
            ),
            ("running call", bash_call("running", None)),
            (
                "tool output",
                HistoryUpdate::ToolOutput {
                    call_id: "running".into(),
                    chunk: "partial".into(),
                },
            ),
            (
                "tool failed",
                HistoryUpdate::ToolCallFailed {
                    call_id: "running".into(),
                    error: "gone".into(),
                },
            ),
            (
                "tool failed again",
                HistoryUpdate::ToolCallFailed {
                    call_id: "running".into(),
                    error: "gone".into(),
                },
            ),
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
                let splice = history.handle(event).unwrap();
                let splice = splice.map_or("none".into(), |s| {
                    format!("{}..{} -> {}", s.range.start, s.range.end, s.len)
                });
                format!("{label}: {splice}")
            })
            .collect::<Vec<_>>()
            .join("\n");

        insta::assert_snapshot!(report, @"
        user: 0..0 -> 1
        created: 1..1 -> 1
        started: 1..2 -> 1
        resolved call: 1..2 -> 1
        completed: 1..2 -> 1
        created: 2..2 -> 1
        running call: 2..3 -> 1
        tool output: 2..3 -> 1
        tool failed: 2..3 -> 1
        tool failed again: none
        fail unresolved: 2..3 -> 1
        fail again: none
        pop: 1..3 -> 0
        pop none: none
        ");
    }

    fn outcomes(history: &History) -> Vec<(String, Vec<Option<String>>)> {
        history
            .state()
            .iter()
            .filter_map(|m| m.try_as_assistant_ref())
            .map(|m| {
                let calls = m.content.values().filter_map(|item| match item {
                    AssistantItem::ToolCall(call) => Some(call.task.output()),
                    _ => None,
                });
                (format!("{:?}", m.status), calls.collect())
            })
            .collect()
    }

    /// a turn error leaves the tool running: an abort then fails the call,
    /// and the response keeps its first error
    #[test]
    fn abort_after_turn_error_fails_running_call() {
        let mut history = History::new(String::new());
        for event in [
            response(AssistantEvent::Created { created_at: 1 }),
            bash_call("running", None),
            response(AssistantEvent::Failed {
                message: "turn error".into(),
                ended_at: 2,
            }),
        ] {
            history.handle(event).unwrap();
        }
        let splice = history
            .handle(HistoryUpdate::FailUnresolved {
                message: "aborted".into(),
                ended_at: 3,
            })
            .unwrap();
        assert_eq!(splice, Some(Splice::edit(0)));
        insta::assert_yaml_snapshot!(outcomes(&history), @r#"
        - - "Error(\"turn error\")"
          - - "{\"error\":\"aborted\"}"
        "#);
    }

    /// failing the last response leaves earlier ones to the restart repair,
    /// which settles every message
    #[test]
    fn only_restart_repair_reaches_earlier_messages() {
        let mut history = History::new(String::new());
        for event in [
            response(AssistantEvent::Created { created_at: 1 }),
            bash_call("running", None),
            user("next"),
            response(AssistantEvent::Created { created_at: 2 }),
            HistoryUpdate::FailUnresolved {
                message: "aborted".into(),
                ended_at: 3,
            },
        ] {
            history.handle(event).unwrap();
        }
        insta::assert_yaml_snapshot!(outcomes(&history), @r#"
        - - Queued
          - - ~
        - - "Error(\"aborted\")"
          - []
        "#);
        history.fail_all_unresolved("lost", 4);
        insta::assert_yaml_snapshot!(outcomes(&history), @r#"
        - - "Error(\"lost\")"
          - - "{\"error\":\"lost\"}"
        - - "Error(\"aborted\")"
          - []
        "#);
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

use anyhow::Context;
use anyhow::Result;
use derive_more::AsMut;
use derive_more::AsRef;
use derive_more::Deref;
use derive_more::DerefMut;
use serde::Deserialize;
use serde::Serialize;

use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::AssistantStatus;
use crate::llm::history::message::Message;
use crate::llm::history::message::ToolCallItem;
use crate::llm::history::tokens::TokenCount;

// TODO drop deref and derefmut

#[derive(Default, Clone, Serialize, Deserialize, Debug, Deref, DerefMut, AsRef, AsMut)]
pub struct HistoryState {
    #[deref]
    #[deref_mut]
    pub messages: Vec<Message>,
    pub token_count: usize,
}

impl TokenCount for HistoryState {
    fn recount(&mut self) {
        self.iter_mut().for_each(TokenCount::recount);
        self.recount_shallow();
    }

    fn token_count(&self) -> usize {
        self.token_count
    }
}

impl From<Vec<Message>> for HistoryState {
    fn from(messages: Vec<Message>) -> Self {
        let mut result = Self {
            messages,
            token_count: 0,
        };
        result.recount_shallow();
        result
    }
}

impl HistoryState {
    pub fn recount_shallow(&mut self) {
        self.token_count = self.iter().map(TokenCount::token_count).sum();
    }

    /// after an edit of the last message
    pub fn recount_last(&mut self) {
        if let Some(last) = self.last_mut() {
            last.recount();
        }
        self.recount_shallow();
    }

    pub fn push(
        &mut self,
        message: Message,
    ) {
        self.messages.push(message);
        self.recount_shallow();
    }

    pub fn ends_with_tool_calls(&self) -> bool {
        self.last().is_some_and(|message| match message {
            Message::Assistant(msg) => {
                matches!(msg.status, AssistantStatus::Success)
                    && msg
                        .content
                        .iter()
                        .any(|(_, content)| matches!(content, AssistantItem::ToolCall(_)))
            }
            Message::Developer(_) | Message::User(_) => false,
        })
    }

    /// an in-flight call is always in the last message: whatever would
    /// follow it waits for the turn to end
    fn live_tool_call_mut(
        &mut self,
        call_id: &str,
    ) -> Result<&mut ToolCallItem> {
        self.last_assistant_msg_mut()?
            .content
            .values_mut()
            .find_map(|item| match item {
                AssistantItem::ToolCall(call) if call.call_id == call_id => Some(call),
                _ => None,
            })
            .with_context(|| format!("no tool call {call_id}"))
    }

    /// fail the matching tool call; returns whether it changed anything
    pub fn fail_tool_call(
        &mut self,
        call_id: &str,
        msg: &str,
    ) -> Result<bool> {
        let failed = self.live_tool_call_mut(call_id)?.fail(msg);
        self.recount_last();
        Ok(failed)
    }

    pub fn push_tool_output(
        &mut self,
        call_id: &str,
        chunk: &str,
    ) -> Result<()> {
        self.live_tool_call_mut(call_id)?.streamed.push_str(chunk);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use crate::agent::tool::generic::ToolOutcome;
    use crate::llm::history::AssistantEvent;
    use crate::llm::history::History;
    use crate::llm::history::HistoryUpdate;
    use crate::llm::history::Splice;
    use crate::llm::history::message::AssistantItem;
    use crate::llm::history::message::ToolCallItem;
    use crate::llm::history::message::UserMessage;
    use crate::llm::history::tokens::TokenCount;
    use crate::llm::history::tokens::count_text_tokens;
    use crate::tools::bash::BashArguments;
    use crate::tools::bash::BashCall;
    use crate::tools::bash::BashResult;

    #[test]
    fn user_message_updates_token_cache() {
        let mut history = History::new(String::new());
        history
            .handle(HistoryUpdate::UserMessage(UserMessage::new(
                "hello".into(),
                0,
            )))
            .unwrap();
        assert_eq!(
            history.state().token_count(),
            10 + count_text_tokens("hello")
        );
    }

    fn bash_call(output: Option<ToolOutcome<BashResult>>) -> HistoryUpdate {
        HistoryUpdate::TurnResponse(AssistantEvent::Item(Box::new(AssistantItem::ToolCall(
            ToolCallItem {
                id: Some("call_1".into()),
                call_id: "call_1".into(),
                task: Box::new(BashCall {
                    arguments: Some(BashArguments {
                        command: "make".into(),
                    }),
                    meta: None,
                    output,
                }),
                streamed: String::new(),
                token_count: 0,
                started_at: 1,
                ended_at: None,
                ready_at: None,
            },
        ))))
    }

    fn user(text: &str) -> HistoryUpdate {
        HistoryUpdate::UserMessage(UserMessage::new(text.into(), 0))
    }

    fn tool_output(
        call_id: &str,
        chunk: &str,
    ) -> HistoryUpdate {
        HistoryUpdate::ToolOutput {
            call_id: call_id.into(),
            chunk: chunk.into(),
        }
    }

    /// chunks gather on the in-flight call; resolving it folds them in
    #[test]
    fn streamed_output_folds_into_the_resolved_call() {
        let mut history = History::new(String::new());
        let result = BashResult {
            output: String::new(),
            exit_status: Some(0),
            signal: None,
        };
        for update in [
            HistoryUpdate::TurnResponse(AssistantEvent::Created { created_at: 0 }),
            bash_call(None),
            tool_output("call_1", "a"),
            tool_output("call_1", "b"),
        ] {
            history.handle(update).unwrap();
        }
        insta::assert_yaml_snapshot!(history.state().messages[0], @"
        role: assistant
        status: Queued
        content:
          - - call_1
            - ToolCall:
                id: call_1
                call_id: call_1
                name: bash
                arguments:
                  command: make
                meta: ~
                output: ~
                streamed: ab
                token_count: 15
                started_at: 1
                ended_at: ~
                ready_at: ~
        token_count: 15
        created_at: 0
        started_at: ~
        ended_at: ~
        ready_at: ~
        ");
        assert_eq!(
            history.handle(tool_output("call_1", "c")).unwrap(),
            Some(Splice::edit(0))
        );
        history
            .handle(bash_call(Some(ToolOutcome::ok(result))))
            .unwrap();
        insta::assert_yaml_snapshot!(history.state().messages[0], @"
        role: assistant
        status: Queued
        content:
          - - call_1
            - ToolCall:
                id: call_1
                call_id: call_1
                name: bash
                arguments:
                  command: make
                meta: ~
                output:
                  result:
                    output: abc
                    exit_status: 0
                    signal: ~
                  error: ~
                token_count: 29
                started_at: 1
                ended_at: ~
                ready_at: ~
        token_count: 29
        created_at: 0
        started_at: ~
        ended_at: ~
        ready_at: ~
        ");
        assert_eq!(
            history
                .handle(tool_output("nope", "c"))
                .unwrap_err()
                .to_string(),
            "no tool call nope"
        );
    }

    /// a tool that returns an error after streaming keeps what it streamed
    #[test]
    fn streamed_output_folds_into_a_returned_error() {
        let mut history = History::new(String::new());
        for update in [
            HistoryUpdate::TurnResponse(AssistantEvent::Created { created_at: 0 }),
            bash_call(None),
            tool_output("call_1", "early"),
            bash_call(Some(ToolOutcome::failed("broken pipe".into()))),
        ] {
            history.handle(update).unwrap();
        }
        insta::assert_yaml_snapshot!(history.state().messages[0], @"
        role: assistant
        status: Queued
        content:
          - - call_1
            - ToolCall:
                id: call_1
                call_id: call_1
                name: bash
                arguments:
                  command: make
                meta: ~
                output:
                  result:
                    output: early
                    exit_status: ~
                    signal: ~
                  error: broken pipe
                token_count: 34
                started_at: 1
                ended_at: ~
                ready_at: ~
        token_count: 34
        created_at: 0
        started_at: ~
        ended_at: ~
        ready_at: ~
        ");
    }

    /// a failure retokenizes its own message, not the rest of the history
    #[test]
    fn failures_recount_only_the_last_message() {
        for failure in [
            HistoryUpdate::ToolCallFailed {
                call_id: "call_1".into(),
                error: "boom".into(),
            },
            HistoryUpdate::FailUnresolved {
                message: "boom".into(),
                ended_at: 2,
            },
        ] {
            let mut history = History::new(String::new());
            for update in [
                user("old"),
                HistoryUpdate::TurnResponse(AssistantEvent::Created { created_at: 0 }),
                bash_call(None),
            ] {
                history.handle(update).unwrap();
            }
            // a deep recount would overwrite this
            history.state.messages[0]
                .try_as_user_mut()
                .unwrap()
                .token_count = 1000;
            history.state.recount_shallow();
            let old = history.state().messages[0].token_count();
            let unfailed = history.state().messages[1].token_count();

            history.handle(failure).unwrap();

            let failed = history.state().messages[1].token_count();
            assert!(failed > unfailed);
            assert_eq!(history.state().messages[0].token_count(), old);
            assert_eq!(history.state().token_count(), old + failed);
        }
    }
}

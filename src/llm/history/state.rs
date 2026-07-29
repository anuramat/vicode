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

    /// every tool call, latest first
    fn tool_calls_mut(&mut self) -> impl Iterator<Item = &mut ToolCallItem> {
        self.messages
            .iter_mut()
            .rev()
            .filter_map(Message::try_as_assistant_mut)
            .flat_map(|m| m.content.values_mut())
            .filter_map(|item| match item {
                AssistantItem::ToolCall(call) => Some(call),
                _ => None,
            })
    }

    /// give matching unresolved tool call(s) an error output, plus whatever
    /// they streamed
    pub fn fail_tool_calls(
        &mut self,
        call_id: Option<&str>,
        msg: &str,
    ) {
        for call in self
            .tool_calls_mut()
            .filter(|call| call_id.is_none_or(|id| call.call_id == id))
        {
            let streamed = std::mem::take(&mut call.streamed);
            call.task.fail_unresolved(&with_partial(msg, &streamed));
        }
        self.recount();
    }

    pub fn push_tool_output(
        &mut self,
        call_id: &str,
        chunk: &str,
    ) -> Result<()> {
        self.tool_calls_mut()
            .find(|call| call.call_id == call_id)
            .with_context(|| format!("no tool call {call_id}"))?
            .streamed
            .push_str(chunk);
        Ok(())
    }
}

/// a failed tool's error: the message plus whatever it streamed first
fn with_partial(
    msg: &str,
    partial: &str,
) -> String {
    if partial.is_empty() {
        msg.into()
    } else {
        format!("{msg}; partial output:\n{partial}")
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use crate::llm::history::AssistantEvent;
    use crate::llm::history::History;
    use crate::llm::history::HistoryUpdate;
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

    fn bash_call(output: Option<Result<BashResult, String>>) -> HistoryUpdate {
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
        history.handle(bash_call(Some(Ok(result)))).unwrap();
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
                  Ok:
                    output: ab
                    exit_status: 0
                    signal: ~
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
}

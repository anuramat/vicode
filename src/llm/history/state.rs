use anyhow::Result;
use derive_more::AsMut;
use derive_more::AsRef;
use derive_more::Deref;
use derive_more::DerefMut;
use derive_more::Display;
use serde::Deserialize;
use serde::Serialize;

use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::AssistantStatus;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::Message;
use crate::llm::history::tokens::TokenCount;

#[derive(Clone, Debug, PartialEq, Eq, Display)]
pub enum TurnStatus {
    #[display("idle")]
    Idle,
    #[display("in progress")]
    InProgress,
    #[display("failed: {_0}")]
    Failed(String),
}

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

    pub fn needs_another_turn(&self) -> bool {
        self.last().is_some_and(|message| match message {
            Message::Assistant(msg) => {
                matches!(msg.status, AssistantStatus::Success)
                    && msg
                        .content
                        .iter()
                        .any(|(_, content)| matches!(content, AssistantItem::ToolCall(_)))
            }
            Message::Developer(msg) => match msg {
                DeveloperMessage::Compact(compact) => compact.needs_another_turn,
                DeveloperMessage::SubagentReport(_) => true,
                DeveloperMessage::Misc(_) => false,
            },
            Message::User(_) => false,
        })
    }

    pub fn last_text_output(&self) -> Result<String> {
        if let Some(Message::Assistant(msg)) = self.last() {
            Ok(msg.text_output())
        } else {
            Err(anyhow::anyhow!("last message is not from the assistant",))
        }
    }

    pub fn text_outputs_after(
        &self,
        n: usize,
    ) -> String {
        self.iter()
            .skip(n)
            .filter_map(|message| match message {
                Message::Assistant(msg) => Some(msg.text_output()),
                _ => None,
            })
            .collect()
    }

    pub fn has_unresolved_tool_calls(&self) -> bool {
        self.last()
            .and_then(|m| m.try_as_assistant_ref())
            .is_some_and(|msg| {
                msg.content.values().any(
                    |item| matches!(item, AssistantItem::ToolCall(t) if t.task.output().is_none()),
                )
            })
    }

    pub fn turn_status(
        &self,
        busy: bool,
    ) -> TurnStatus {
        if busy {
            return TurnStatus::InProgress;
        }
        if self.has_unresolved_tool_calls() {
            return TurnStatus::Failed("tool calls not resolved".into());
        }
        self.status()
            .map_or(TurnStatus::Idle, |status| match status {
                AssistantStatus::Queued => TurnStatus::Failed(
                    "last assistant message is queued but no tasks are running".into(),
                ),
                AssistantStatus::InProgress => TurnStatus::Failed(
                    "last assistant message is in progress but no tasks are running".into(),
                ),
                AssistantStatus::Success => TurnStatus::Idle,
                AssistantStatus::Error(e) => TurnStatus::Failed(e),
            })
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use crate::llm::history::History;
    use crate::llm::history::HistoryUpdate;
    use crate::llm::history::tokens::TokenCount;
    use crate::llm::history::tokens::count_text_tokens;

    #[test]
    fn user_message_updates_token_cache() {
        let mut history = History::new(String::new());
        history
            .handle(0, HistoryUpdate::UserMessage("hello".into()))
            .unwrap();
        assert_eq!(
            history.state().token_count(),
            10 + count_text_tokens("hello")
        );
    }
}

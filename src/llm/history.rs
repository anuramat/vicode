use anyhow::Result;
use indexmap::indexmap;
use serde::Deserialize;
use serde::Serialize;

use crate::llm::delta::Delta;
use crate::llm::message::AssistantItem;
use crate::llm::message::AssistantMessage;
use crate::llm::message::AssistantMessageStatus;
use crate::llm::message::DeveloperMessage;
use crate::llm::message::Message;
use crate::llm::message::UserMessage;
use crate::tui::widgets::container::composite::CompositeElement;
use crate::tui::widgets::container::element::Element;

#[derive(Default, Clone, Serialize, Deserialize, Debug)]
pub struct History {
    messages: Vec<Message>,
}

// TODO this is kinda ugly, maybe send usize + HistoryEvent tuple?
#[derive(Debug, Clone)]
pub enum HistoryEvent {
    ResponseDelta(usize, Delta),
    ResponseItem(usize, Box<AssistantItem>),
    ResponseCompleted(usize, Vec<AssistantItem>),
    ResponseFailed(usize, String),
    UserMessage(usize, String),
    DeveloperMessage(usize, String),
}

impl AsRef<[Message]> for History {
    fn as_ref(&self) -> &[Message] {
        &self.messages
    }
}

impl History {
    pub fn messages(self) -> Vec<Message> {
        self.messages
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_mut(
        &mut self,
        loc: usize,
    ) -> Option<&mut Message> {
        self.messages.get_mut(loc)
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn get(
        &mut self,
        loc: usize,
    ) -> Option<&Message> {
        self.messages.get(loc)
    }

    pub fn needs_another_turn(&self) -> bool {
        if let Some(msg) = self.messages.last() {
            match msg {
                Message::Assistant(msg) => msg
                    .content
                    .iter()
                    .any(|(_, content)| content.try_as_tool_call_ref().is_some()),
                Message::Developer(_) => true,
                Message::User(_) => false,
            }
        } else {
            false
        }
    }

    pub fn handle(
        &mut self,
        event: HistoryEvent,
    ) {
        match event {
            HistoryEvent::ResponseDelta(loc, item_delta) => {
                self.push_delta(loc, item_delta);
            }
            HistoryEvent::ResponseItem(loc, item) => {
                self.push_item(loc, *item);
            }
            HistoryEvent::ResponseCompleted(loc, items) => {
                self.complete_response(loc, items);
            }
            HistoryEvent::ResponseFailed(loc, msg) => {
                self.fail_response(loc, msg);
            }
            HistoryEvent::DeveloperMessage(loc, text) => {
                // TODO unslop this
                if loc == self.messages.len() {
                    let msg = Message::Developer(DeveloperMessage { text });
                    self.messages.push(msg)
                } else {
                    panic!(
                        "DeveloperMessage location {} does not match history length {} in {:?}",
                        loc,
                        self.messages.len(),
                        self.messages,
                    );
                }
            }
            HistoryEvent::UserMessage(loc, text) => {
                if loc == self.messages.len() {
                    let msg = Message::User(UserMessage { text });
                    self.messages.push(msg)
                } else {
                    panic!(
                        "UserMessage location {} does not match history length {} in {:?}",
                        loc,
                        self.messages.len(),
                        self.messages,
                    );
                }
            }
        }
    }

    pub fn push_item(
        &mut self,
        loc: usize,
        mut item: AssistantItem,
    ) {
        if let Some(Message::Assistant(msg)) = self.messages.get_mut(loc) {
            // if item already exists -- replace it but preserve start
            // if item has finish, it means that we constructed it from delta, the new item is
            // just for consistency guarantee, and thus we actually finished the
            // existing item when the last delta arrived, so we preserve the smaller finish value
            if let Some(existing) = msg.content.get(&item.id()) {
                item.timing_mut().started_at_ms = existing.timing().started_at_ms;
                if let Some(modified) = existing.timing().last_modified_ms {
                    item.timing_mut().last_modified_ms = Some(modified);
                }
            }
            _ = msg.content.insert(item.id(), item);
        } else if loc == self.messages.len() {
            let msg = AssistantMessage {
                finish_reason: AssistantMessageStatus::Success,
                content: indexmap! {item.id() => item},
            };
            self.messages.push(msg.into());
        }
    }

    pub fn complete_response(
        &mut self,
        loc: usize,
        _items: Vec<AssistantItem>,
    ) {
        if let Some(Message::Assistant(msg)) = self.messages.get_mut(loc) {
            msg.finish_reason = AssistantMessageStatus::Success;
        }
    }

    pub fn fail_response(
        &mut self,
        loc: usize,
        error_text: String, // TODO rename to msg or whatever
    ) {
        if let Some(Message::Assistant(msg)) = self.messages.get_mut(loc) {
            msg.finish_reason = AssistantMessageStatus::Error(error_text);
        }
    }

    pub fn last_output(&self) -> Result<String> {
        if let Some(Message::Assistant(msg)) = self.messages.last() {
            Ok(msg.output())
        } else {
            Err(anyhow::anyhow!("last message is not from the assistant",))
        }
    }

    pub fn without_last_assistant_tool_calls(&self) -> Self {
        let mut history = self.clone();
        let should_pop = matches!(
            history.messages.last(),
            Some(Message::Assistant(msg))
                if msg.content.values().any(|item| item.try_as_tool_call_ref().is_some())
        );
        if should_pop {
            history.messages.pop();
        }
        history
    }
}

impl From<&History> for Vec<Message> {
    fn from(history: &History) -> Self {
        history.messages.clone()
    }
}

impl From<&History> for CompositeElement {
    fn from(history: &History) -> Self {
        let vec: Vec<Element> = history.messages.iter().map(|m| m.into()).collect();
        CompositeElement(vec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::message::ItemTiming;
    use crate::llm::message::OutputItem;

    #[test]
    fn response_starts_without_assistant_message() {
        let history = History::new();
        assert!(history.messages.is_empty());
    }

    #[test]
    fn response_failed_without_message_is_ignored() {
        let mut history = History::new();
        history.handle(HistoryEvent::ResponseFailed(0, "oops".into()));
        assert!(history.messages.is_empty());
    }

    #[test]
    fn response_completed_marks_message_success() {
        let mut history = History::new();
        history.handle(HistoryEvent::ResponseItem(
            0,
            Box::new(AssistantItem::Output(OutputItem {
                id: "out".into(),
                timing: ItemTiming::new(),
                content: vec![],
            })),
        ));
        history.handle(HistoryEvent::ResponseCompleted(
            0,
            vec![AssistantItem::Output(OutputItem {
                id: "out".into(),
                timing: ItemTiming {
                    started_at_ms: 1,
                    last_modified_ms: Some(2),
                },
                content: vec![],
            })],
        ));
        let Some(Message::Assistant(msg)) = history.messages.first() else {
            panic!("expected assistant message");
        };
        let item = msg.content.get("out").unwrap().try_as_output_ref().unwrap();
        assert_eq!(item.timing.last_modified_ms, None);
        assert!(matches!(msg.finish_reason, AssistantMessageStatus::Success));
    }
}

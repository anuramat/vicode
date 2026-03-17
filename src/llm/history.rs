use anyhow::Result;
use indexmap::indexmap;
use serde::Deserialize;
use serde::Serialize;

use crate::llm::delta::Delta;
use crate::llm::message::AssistantItem;
use crate::llm::message::AssistantMessage;
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
        item: AssistantItem,
    ) {
        if let Some(Message::Assistant(msg)) = self.messages.get_mut(loc) {
            _ = msg.content.insert(item.id(), item);
        } else if loc == self.messages.len() {
            let msg = AssistantMessage {
                content: indexmap! {item.id() => item},
            };
            self.messages.push(msg.into());
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

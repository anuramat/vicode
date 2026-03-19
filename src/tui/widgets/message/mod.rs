use crate::llm::message::AssistantItem;
use crate::llm::message::AssistantMessage;
use crate::llm::message::HistoryEntry;
use crate::llm::message::Message;
use crate::tui::widgets::container::composite::CompositeElement;
use crate::tui::widgets::container::element::*;

pub mod developer;
pub mod output;
pub mod reasoning;
pub mod toolcall;
pub mod user;

impl From<&HistoryEntry> for Element {
    fn from(entry: &HistoryEntry) -> Self {
        (&entry.message).into()
    }
}

impl From<&Message> for Element {
    fn from(message: &Message) -> Self {
        match message {
            Message::Developer(developer) => developer.into(),
            Message::User(user) => user.into(),
            Message::Assistant(assistant) => assistant.into(),
        }
    }
}

impl From<&AssistantMessage> for Element {
    fn from(value: &AssistantMessage) -> Self {
        CompositeElement(value.content.values().map(Element::from).collect()).into()
    }
}

impl From<&AssistantItem> for Element {
    fn from(content: &AssistantItem) -> Self {
        match content {
            AssistantItem::Output(x) => x.into(),
            AssistantItem::Reasoning(x) => x.into(),
            AssistantItem::ToolCall(x) => x.into(),
        }
    }
}

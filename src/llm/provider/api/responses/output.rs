use async_openai::types::responses::OutputTextContent;
use async_openai::types::responses::{self};

use crate::llm::history::message::OutputContent;
use crate::llm::history::message::OutputItem;
use crate::utils::now;

impl From<&OutputItem> for responses::InputItem {
    fn from(item: &OutputItem) -> Self {
        let text: String = item
            .content
            .iter()
            .map(|v| match v {
                OutputContent::Text(text) => text.clone(),
                OutputContent::Refusal(_) => unimplemented!("refusal in an assistant message"),
            })
            .collect();
        let item = responses::MessageItem::Output(responses::OutputMessage {
            id: item.id.clone(),
            content: vec![responses::OutputMessageContent::OutputText(
                OutputTextContent {
                    annotations: Vec::new(),
                    logprobs: None,
                    text,
                },
            )],
            role: responses::AssistantRole::Assistant,
            status: responses::OutputStatus::Completed,
        });
        Self::Item(responses::Item::Message(item))
    }
}

impl From<responses::OutputMessage> for OutputItem {
    fn from(value: responses::OutputMessage) -> Self {
        let mut item = Self::new(value.id, now());
        item.content = value
            .content
            .into_iter()
            .map(|c| match c {
                responses::OutputMessageContent::OutputText(t) => OutputContent::Text(t.text),
                responses::OutputMessageContent::Refusal(r) => OutputContent::Refusal(r.refusal),
            })
            .collect();
        item
    }
}

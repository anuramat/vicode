use async_openai::types::responses::OutputTextContent;
use async_openai::types::responses::{self};

use crate::llm::message::*;

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
        // if text.trim().is_empty() {
        //     // TODO z.ai sends "\n" messages and breaks when we send it back, thus we drop empty items; move this to compat
        //     return None;
        // }
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
        responses::InputItem::Item(responses::Item::Message(item))
    }
}

impl From<responses::OutputMessage> for OutputItem {
    fn from(value: responses::OutputMessage) -> Self {
        Self {
            id: value.id,
            started_at_ms: now_ms(),
            finished_at_ms: None,
            content: value
                .content
                .into_iter()
                .map(|c| match c {
                    responses::OutputMessageContent::OutputText(t) => OutputContent::Text(t.text),
                    responses::OutputMessageContent::Refusal(r) => {
                        OutputContent::Refusal(r.refusal)
                    }
                })
                .collect(),
        }
    }
}

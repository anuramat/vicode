use async_openai::types::responses;

use crate::llm::message::ReasoningItem;

impl From<responses::ReasoningItem> for ReasoningItem {
    fn from(item: responses::ReasoningItem) -> Self {
        Self {
            id: item.id,
            content: item
                .content
                .map(|c| c.into_iter().map(|s| s.text).collect()),
            summary: item
                .summary
                .into_iter()
                .map(|responses::SummaryPart::SummaryText(s)| s.text)
                .collect(),
            encrypted: item.encrypted_content,
        }
    }
}

impl From<ReasoningItem> for responses::InputItem {
    fn from(item: ReasoningItem) -> Self {
        responses::InputItem::Item(responses::Item::Reasoning(responses::ReasoningItem {
            id: item.id,
            summary: item
                .summary
                .into_iter()
                .map(|text| {
                    responses::SummaryPart::SummaryText(responses::SummaryTextContent { text })
                })
                .collect(),
            content: item.content.map(|content| {
                content
                    .into_iter()
                    .map(|text| responses::ReasoningTextContent { text })
                    .collect()
            }),
            encrypted_content: item.encrypted,
            status: None,
        }))
    }
}

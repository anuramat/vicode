use async_openai::types::responses;

use crate::llm::history::message::ReasoningItem;

impl From<responses::ReasoningItem> for ReasoningItem {
    fn from(item: responses::ReasoningItem) -> Self {
        let mut result = Self::new(item.id);
        result.content = item
            .content
            .map(|c| c.into_iter().map(|s| s.text).collect());
        result.summary = item
            .summary
            .into_iter()
            .map(|responses::SummaryPart::SummaryText(s)| s.text)
            .collect();
        result.encrypted = item.encrypted_content;
        result
    }
}

impl From<ReasoningItem> for responses::InputItem {
    fn from(item: ReasoningItem) -> Self {
        Self::Item(responses::Item::Reasoning(responses::ReasoningItem {
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

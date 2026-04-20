use std::fmt::Write;

use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::AssistantMessage;
use crate::llm::history::message::Message;
use crate::llm::history::message::OutputItem;
use crate::llm::history::message::ReasoningItem;

pub fn reasoning_to_output(
    tag: &str,
    msg: &mut Message,
) {
    fn transform_item(
        tag: &str,
        reasoning_item: &ReasoningItem,
    ) -> OutputItem {
        let mut item = OutputItem {
            id: String::new(), // WARN empty id might be a problem
            started_at: reasoning_item.started_at,
            ended_at: reasoning_item.ended_at,
            token_count: reasoning_item.token_count,
            content: vec![],
        };
        if let Some(reasoning_content) = &reasoning_item.content {
            let mut text: String = format!("<{tag}>");
            for x in reasoning_content {
                text.push_str(x);
            }
            write!(text, "</{tag}>").expect("failed to write closing tag");
            item.content
                .push(crate::llm::history::message::OutputContent::Text(text));
        }
        item
    }

    if let Message::Assistant(AssistantMessage { content, .. }) = msg {
        content.iter_mut().for_each(|(_, item)| {
            if let AssistantItem::Reasoning(reasoning_item) = item {
                *item = AssistantItem::Output(transform_item(tag, reasoning_item));
            }
        });
    }
}

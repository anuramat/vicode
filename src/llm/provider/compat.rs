use crate::llm::message::*;

pub fn reasoning_to_output(
    tag: &str,
    msg: &mut Message,
) {
    fn transform_item(
        tag: &str,
        reasoning_item: &ReasoningItem,
    ) -> OutputItem {
        let mut item = OutputItem {
            id: String::new(), // WARN might be a problem
            content: vec![],
        };
        if let Some(reasoning_content) = &reasoning_item.content {
            let mut text: String = format!("<{}>", tag);
            reasoning_content.iter().for_each(|x| text.push_str(x));
            text.push_str(&format!("</{}>", tag));
            item.content
                .push(crate::llm::message::OutputContent::Text(text));
        };
        item
    }

    if let Some(msg) = msg.try_as_assistant_mut() {
        msg.content.iter_mut().for_each(|(_, item)| {
            if let AssistantItem::Reasoning(reasoning_item) = item {
                *item = AssistantItem::Output(transform_item(tag, reasoning_item));
            }
        });
    };
}

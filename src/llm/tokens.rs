use tiktoken_rs::CoreBPE;
use tiktoken_rs::o200k_base;

use crate::llm::message::AsMessageText;
use crate::llm::message::AssistantItem;
use crate::llm::message::Message;
use crate::llm::message::OutputContent;
use crate::llm::message::ToolCallItem;

static BPE: std::sync::LazyLock<CoreBPE> = std::sync::LazyLock::new(|| o200k_base().unwrap());

pub fn count_text_tokens(text: &str) -> usize {
    BPE.encode_with_special_tokens(text).len()
}

pub fn count_message_tokens(message: &Message) -> usize {
    10 + match message {
        Message::Developer(msg) => count_text_tokens(&msg.as_message_text()),
        Message::User(msg) => count_text_tokens(&msg.text),
        Message::Assistant(msg) => msg.content.values().map(count_assistant_item_tokens).sum(),
    }
}

fn count_assistant_item_tokens(item: &AssistantItem) -> usize {
    match item {
        AssistantItem::Output(item) => item
            .content
            .iter()
            .map(|content| match content {
                OutputContent::Text(text) | OutputContent::Refusal(text) => count_text_tokens(text),
            })
            .sum(),
        AssistantItem::Reasoning(item) => item
            .content
            .as_ref()
            .map_or(0, |content| count_text_tokens(&content.join(""))),
        AssistantItem::ToolCall(item) => 10 + count_tool_call_tokens(item),
    }
}

fn count_tool_call_tokens(item: &ToolCallItem) -> usize {
    count_text_tokens(&item.task.arguments())
        + item
            .task
            .output()
            .map_or(0, |output| count_text_tokens(&output))
}

#[cfg(test)]
mod tests {
    use indexmap::indexmap;
    use similar_asserts::assert_eq;

    use super::*;
    use crate::llm::message::AssistantMessage;
    use crate::llm::message::ItemTiming;
    use crate::llm::message::OutputItem;
    use crate::tools::bash::BashArguments;
    use crate::tools::bash::BashCall;
    use crate::tools::bash::BashResult;

    fn tool_call(output: Option<anyhow::Result<BashResult, String>>) -> ToolCallItem {
        ToolCallItem {
            id: Some("call_1".into()),
            call_id: "call_1".into(),
            timing: ItemTiming::new(),
            executed_at_ms: None,
            task: Box::new(BashCall {
                arguments: Some(BashArguments {
                    command: "echo hello".into(),
                }),
                output,
                meta: None,
                context: None,
            }),
        }
    }

    #[test]
    fn counts_user_message_tokens() {
        let message = Message::User(crate::llm::message::UserMessage {
            text: "hello world".into(),
        });
        assert_eq!(
            count_message_tokens(&message),
            10 + count_text_tokens("hello world")
        );
    }

    #[test]
    fn counts_assistant_output_and_refusal_tokens() {
        let message = Message::Assistant(AssistantMessage {
            finish_reason: Default::default(),
            content: indexmap! {
                "out".into() => AssistantItem::Output(OutputItem {
                    id: "out".into(),
                    timing: ItemTiming::new(),
                    content: vec![
                        OutputContent::Text("hello".into()),
                        OutputContent::Refusal("nope".into()),
                    ],
                }),
            },
        });
        assert_eq!(
            count_message_tokens(&message),
            10 + count_text_tokens("hello") + count_text_tokens("nope")
        );
    }

    #[test]
    fn counts_tool_call_arguments_and_output_tokens() {
        let call = tool_call(Some(Ok(BashResult {
            stdout: "hello\n".into(),
            stderr: String::new(),
            exit_status: None,
            signal: None,
        })));
        let expected = 10
            + 10
            + count_text_tokens(&call.task.arguments())
            + count_text_tokens(&call.task.output().unwrap());
        let message = Message::Assistant(AssistantMessage {
            finish_reason: Default::default(),
            content: indexmap! {
                "call".into() => AssistantItem::ToolCall(call),
            },
        });
        assert_eq!(count_message_tokens(&message), expected);
    }
}

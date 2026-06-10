use ambassador::delegatable_trait;
use tiktoken_rs::CoreBPE;
use tiktoken_rs::o200k_base;

// TODO instead of unwrap, fall back to chars/3, and log an error
static BPE: std::sync::LazyLock<CoreBPE> = std::sync::LazyLock::new(|| o200k_base().unwrap());
pub const MESSAGE_OVERHEAD: usize = 10;
pub const TOOLCALL_OVERHEAD: usize = 10;

pub fn count_text_tokens(text: &str) -> usize {
    BPE.encode_with_special_tokens(text).len()
}

#[delegatable_trait]
pub trait TokenCount {
    fn recount(&mut self);
    fn token_count(&self) -> usize;
}

#[cfg(test)]
mod tests {
    use indexmap::indexmap;
    use similar_asserts::assert_eq;

    use super::*;
    use crate::llm::history::message::AssistantItem;
    use crate::llm::history::message::AssistantMessage;
    use crate::llm::history::message::Message;
    use crate::llm::history::message::OutputContent;
    use crate::llm::history::message::OutputItem;
    use crate::llm::history::message::ToolCallItem;
    use crate::llm::history::message::UserMessage;
    use crate::tools::bash::BashArguments;
    use crate::tools::bash::BashCall;
    use crate::tools::bash::BashResult;

    fn tool_call(output: Option<anyhow::Result<BashResult, String>>) -> ToolCallItem {
        ToolCallItem {
            id: Some("call_1".into()),
            call_id: "call_1".into(),
            started_at: 0,
            ended_at: None,
            ready_at: None,
            token_count: 0,
            task: Box::new(BashCall {
                arguments: Some(BashArguments {
                    command: "echo hello".into(),
                }),
                output,
                meta: None,
            }),
        }
    }

    fn assistant_with(content: indexmap::IndexMap<String, AssistantItem>) -> Message {
        let mut msg = AssistantMessage::new(0);
        msg.content = content;
        let mut m: Message = msg.into();
        m.recount();
        m
    }

    #[test]
    fn counts_user_message_tokens() {
        let mut msg: Message = UserMessage::new("hello world".into(), 0).into();
        msg.recount();
        assert_eq!(
            msg.token_count(),
            MESSAGE_OVERHEAD + count_text_tokens("hello world")
        );
    }

    #[test]
    fn counts_assistant_output_and_refusal_tokens() {
        let msg = assistant_with(indexmap! {
            "out".into() => AssistantItem::Output(OutputItem {
                id: "out".into(),
                started_at: 0,
                ended_at: None,
                token_count: 0,
                content: vec![
                    OutputContent::Text("hello".into()),
                    OutputContent::Refusal("nope".into()),
                ],
            }),
        });
        assert_eq!(
            msg.token_count(),
            MESSAGE_OVERHEAD + count_text_tokens("hello") + count_text_tokens("nope")
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
        let expected = MESSAGE_OVERHEAD
            + TOOLCALL_OVERHEAD
            + count_text_tokens(&call.task.arguments())
            + count_text_tokens(&call.task.output().unwrap());
        let msg = assistant_with(indexmap! {
            "call".into() => AssistantItem::ToolCall(call),
        });
        assert_eq!(msg.token_count(), expected);
    }
}

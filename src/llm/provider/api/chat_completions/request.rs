use anyhow::Result;
use async_openai::types::chat::ChatCompletionMessageToolCall;
use async_openai::types::chat::ChatCompletionMessageToolCalls;
use async_openai::types::chat::ChatCompletionRequestAssistantMessage;
use async_openai::types::chat::ChatCompletionRequestAssistantMessageContent;
use async_openai::types::chat::ChatCompletionRequestMessage;
use async_openai::types::chat::ChatCompletionRequestSystemMessage;
use async_openai::types::chat::ChatCompletionRequestToolMessage;
use async_openai::types::chat::ChatCompletionRequestToolMessageContent;
use async_openai::types::chat::ChatCompletionRequestUserMessage;
use async_openai::types::chat::CreateChatCompletionRequestArgs;

use crate::agent::tool::registry::ToolSchemas;
use crate::config::ApiCompatConfig;
use crate::config::ModelConfig;
use crate::llm::message::AsMessageText;
use crate::llm::message::AssistantItem;
use crate::llm::message::AssistantMessage;
use crate::llm::message::DeveloperMessage;
use crate::llm::message::Message;
use crate::llm::message::OutputContent;
use crate::llm::message::OutputItem;
use crate::llm::message::ToolCallItem;
use crate::llm::message::UserMessage;

pub fn request(
    assistant: ModelConfig,
    instructions: String,
    mut items: Vec<Message>,
    tools: ToolSchemas,
    streaming: bool,
    compat: &ApiCompatConfig,
) -> Result<serde_json::Value> {
    let mut builder = CreateChatCompletionRequestArgs::default();
    builder.model(&assistant.model).parallel_tool_calls(true);
    if let Some(effort) = assistant.effort {
        builder.reasoning_effort(effort);
    }
    items.insert(0, Message::Developer(DeveloperMessage::new(instructions)));

    if let Some(tag) = compat.reasoning_as_output.clone() {
        items.iter_mut().for_each(move |message| {
            crate::llm::provider::compat::reasoning_to_output(&tag, message)
        });
    }

    if compat.developer_as_user {
        items.iter_mut().for_each(|message| {
            if let Message::Developer(dev_msg) = message {
                *message = Message::User(UserMessage {
                    text: dev_msg.as_message_text(),
                });
            }
        });
    }

    let message_values = message_values(&items, &compat.reasoning_content_field);

    builder.stream(streaming).messages(messages(items));
    if !tools.0.is_empty() {
        builder.tools(Vec::from(tools));
    }
    let mut value = serde_json::to_value(builder.build()?)?;
    value["messages"] = serde_json::Value::Array(message_values);
    Ok(value)
}

fn messages(history: impl IntoIterator<Item = Message>) -> Vec<ChatCompletionRequestMessage> {
    history
        .into_iter()
        .flat_map(|message| from_message(&message))
        .collect()
}

fn from_message(message: &Message) -> Vec<ChatCompletionRequestMessage> {
    match message {
        Message::Developer(msg) => vec![ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: msg.as_message_text().into(),
                name: None,
            },
        )],
        Message::User(msg) => vec![ChatCompletionRequestMessage::User(
            ChatCompletionRequestUserMessage {
                content: msg.text.clone().into(),
                name: None,
            },
        )],
        Message::Assistant(msg) => assistant_messages(msg),
    }
}

fn assistant_messages(message: &AssistantMessage) -> Vec<ChatCompletionRequestMessage> {
    message
        .content
        .values()
        .flat_map(|item| match item {
            AssistantItem::Output(item) => output_messages(item),
            AssistantItem::Reasoning(_) => Vec::new(),
            AssistantItem::ToolCall(item) => tool_call_messages(item),
        })
        .collect()
}

fn message_values(
    messages: &[Message],
    reasoning_content_field: &str,
) -> Vec<serde_json::Value> {
    messages
        .iter()
        .flat_map(|message| match message {
            Message::Assistant(msg) => assistant_values(msg, reasoning_content_field),
            _ => from_message(message)
                .into_iter()
                .map(|m| serde_json::to_value(m).unwrap())
                .collect(),
        })
        .collect()
}

fn assistant_values(
    message: &AssistantMessage,
    reasoning_content_field: &str,
) -> Vec<serde_json::Value> {
    let reasoning: String = message
        .content
        .values()
        .filter_map(|item| match item {
            AssistantItem::Reasoning(r) => r.content.as_ref(),
            _ => None,
        })
        .flatten()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("");

    message
        .content
        .values()
        .flat_map(|item| -> Vec<serde_json::Value> {
            match item {
                AssistantItem::Output(item) => output_messages(item)
                    .into_iter()
                    .map(|m| {
                        let mut v = serde_json::to_value(m).unwrap();
                        if let (true, Some(obj)) = (!reasoning.is_empty(), v.as_object_mut()) {
                            obj.insert(
                                reasoning_content_field.to_string(),
                                reasoning.clone().into(),
                            );
                        }
                        v
                    })
                    .collect(),
                AssistantItem::Reasoning(_) => vec![],
                AssistantItem::ToolCall(item) => tool_call_messages(item)
                    .into_iter()
                    .map(|m| serde_json::to_value(m).unwrap())
                    .collect(),
            }
        })
        .collect()
}

fn output_messages(item: &OutputItem) -> Vec<ChatCompletionRequestMessage> {
    let text: String = item
        .content
        .iter()
        .filter_map(|content| match content {
            OutputContent::Text(text) => Some(text.as_str()),
            OutputContent::Refusal(_) => None,
        })
        .collect();
    let refusal = item.content.iter().find_map(|content| match content {
        OutputContent::Refusal(text) => Some(text.clone()),
        OutputContent::Text(_) => None,
    });
    if text.is_empty() && refusal.is_none() {
        return Vec::new();
    }
    vec![ChatCompletionRequestMessage::Assistant(
        ChatCompletionRequestAssistantMessage {
            content: (!text.is_empty())
                .then_some(ChatCompletionRequestAssistantMessageContent::Text(text)),
            refusal,
            tool_calls: None,
            ..Default::default()
        },
    )]
}

fn tool_call_messages(item: &ToolCallItem) -> Vec<ChatCompletionRequestMessage> {
    let call_id = item.id().clone();
    let mut messages = vec![ChatCompletionRequestMessage::Assistant(
        ChatCompletionRequestAssistantMessage {
            content: None,
            tool_calls: Some(vec![ChatCompletionMessageToolCalls::Function(
                ChatCompletionMessageToolCall {
                    id: call_id.clone(),
                    function: async_openai::types::chat::FunctionCall {
                        name: item.task.typetag_name().to_string(),
                        arguments: item.task.arguments(),
                    },
                },
            )]),
            ..Default::default()
        },
    )];
    if let Some(output) = item.task.output() {
        messages.push(ChatCompletionRequestMessage::Tool(
            ChatCompletionRequestToolMessage {
                content: ChatCompletionRequestToolMessageContent::Text(output),
                tool_call_id: call_id,
            },
        ));
    }
    messages
}

#[cfg(test)]
mod tests {
    use async_openai::types::chat::ChatCompletionRequestMessage;
    use async_openai::types::chat::ChatCompletionResponseMessage;
    use async_openai::types::chat::Role;
    use indexmap::indexmap;

    use super::messages;
    use crate::llm::message::AssistantItem;
    use crate::llm::message::AssistantMessage;
    use crate::llm::message::ItemTiming;
    use crate::llm::message::Message;
    use crate::llm::message::ToolCallItem;
    use crate::tools::bash::BashArguments;
    use crate::tools::bash::BashCall;
    use crate::tools::bash::BashResult;

    #[test]
    fn assistant_tool_call_round_trip() {
        let task = BashCall {
            arguments: Some(BashArguments {
                command: "echo hello".into(),
            }),
            output: Some(Ok(BashResult {
                stdout: "hello\n".into(),
                stderr: String::new(),
                exit_status: None,
                signal: None,
            })),
            meta: None,
            context: None,
        };
        let message = Message::Assistant(AssistantMessage {
            finish_reason: Default::default(),
            content: indexmap! {
                "call_1".into() => AssistantItem::ToolCall(ToolCallItem {
                    id: Some("call_1".into()),
                    call_id: "call_1".into(),
                    timing: ItemTiming {
                        started_at_ms: 1,
                        last_modified_ms: Some(2),
                    },
                    executed_at_ms: Some(3),
                    task: Box::new(task),
                })
            },
        });

        let messages = messages([message]);
        assert!(matches!(
            &messages[0],
            ChatCompletionRequestMessage::Assistant(_)
        ));
        assert!(matches!(
            &messages[1],
            ChatCompletionRequestMessage::Tool(_)
        ));
    }

    #[test]
    fn output_shape_is_supported() {
        let message = ChatCompletionResponseMessage {
            content: Some("hello".into()),
            refusal: None,
            tool_calls: None,
            annotations: None,
            role: Role::Assistant,
            function_call: None,
            audio: None,
        };
        assert_eq!(message.content.as_deref(), Some("hello"));
    }
}

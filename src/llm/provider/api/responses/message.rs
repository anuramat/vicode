use async_openai::types::responses;

use crate::llm::message::*;

impl From<&Message> for Vec<responses::InputItem> {
    fn from(val: &Message) -> Self {
        match val {
            Message::Developer(m) => vec![m.into()],
            Message::User(m) => vec![m.into()],
            Message::Assistant(m) => m.into(),
        }
    }
}

impl From<&DeveloperMessage> for responses::InputItem {
    fn from(msg: &DeveloperMessage) -> Self {
        responses::InputItem::EasyMessage(responses::EasyInputMessage {
            r#type: responses::MessageType::Message,
            role: responses::Role::Developer,
            content: responses::EasyInputContent::Text(msg.as_message_text()),
        })
    }
}

impl From<&UserMessage> for responses::InputItem {
    fn from(msg: &UserMessage) -> Self {
        let UserMessage { text } = msg;
        let item = responses::MessageItem::Input(responses::InputMessage {
            content: vec![responses::InputContent::InputText(text.into())],
            role: responses::InputRole::User,
            status: None,
        });
        responses::InputItem::Item(responses::Item::Message(item))
    }
}

impl From<&AssistantMessage> for Vec<responses::InputItem> {
    fn from(msg: &AssistantMessage) -> Self {
        msg.content
            .iter()
            .flat_map(|(_, v)| Vec::<responses::InputItem>::from(v))
            .collect()
    }
}

impl From<&AssistantItem> for Vec<responses::InputItem> {
    fn from(item: &AssistantItem) -> Self {
        match item {
            AssistantItem::Output(msg) => vec![msg.into()],
            AssistantItem::Reasoning(reasoning) => {
                vec![reasoning.clone().into()]
            }
            AssistantItem::ToolCall(call) => call.into(),
        }
    }
}

impl TryFrom<responses::OutputItem> for AssistantItem {
    type Error = anyhow::Error;

    fn try_from(item: responses::OutputItem) -> Result<Self, Self::Error> {
        Ok(match item {
            responses::OutputItem::Message(msg) => Self::Output(msg.into()),
            responses::OutputItem::Reasoning(reasoning) => Self::Reasoning(reasoning.into()),
            responses::OutputItem::FunctionCall(call) => Self::ToolCall(call.try_into()?),
            _ => unimplemented!("unsupported OutputItem variant"),
        })
    }
}

#[cfg(test)]
mod tests {
    use async_openai::types::responses::FunctionToolCall;
    use similar_asserts::assert_eq;

    use crate::llm::message::*;
    use crate::tools::bash::BashArguments;
    use crate::tools::bash::BashCall;
    use crate::tools::bash::BashResult;

    #[test]
    fn to_api() {
        let task = BashCall {
            arguments: Some(BashArguments {
                command: "echo hello".into(),
            }),
            output: Some(Ok(BashResult {
                stdout: "hello\n".into(),
                stderr: "".into(),
                exit_status: None,
                signal: None,
            })),
            meta: None,
            context: None,
        };

        let call = ToolCallItem {
            id: Some("id_1".into()),
            call_id: "call_id_2".into(),
            timing: ItemTiming {
                started_at_ms: 1,
                last_modified_ms: Some(2),
            },
            executed_at_ms: Some(3),
            task: Box::new(task),
        };

        let api_call: Vec<async_openai::types::responses::InputItem> = (&call).into();
        let value = serde_json::to_value(&api_call).unwrap();

        let arguments = r#"{"command":"echo hello"}"#;
        let output = r#"{"stdout":"hello\n","stderr":"","exit_status":null,"signal":null}"#;
        let expected = serde_json::json!(
            [
                {"type":"function_call","id":"id_1","call_id":"call_id_2","name":"bash","arguments":arguments},
                {"type":"function_call_output","call_id":"call_id_2","output":output}
            ]
        );

        assert_eq!(value, expected);
    }

    #[test]
    fn to_api_err() {
        let task = BashCall {
            arguments: Some(BashArguments {
                command: "echo hello".into(),
            }),
            output: Some(Err("oops".into())),
            meta: None,
            context: None,
        };

        let call = ToolCallItem {
            id: Some("id_1".into()),
            call_id: "call_id_2".into(),
            timing: ItemTiming {
                started_at_ms: 1,
                last_modified_ms: Some(2),
            },
            executed_at_ms: Some(3),
            task: Box::new(task),
        };

        let api_call: Vec<async_openai::types::responses::InputItem> = (&call).into();
        let value = serde_json::to_value(&api_call).unwrap();

        let arguments = r#"{"command":"echo hello"}"#;
        let output = r#"{"error":"oops"}"#;
        let expected = serde_json::json!(
            [
                {"type":"function_call","id":"id_1","call_id":"call_id_2","name":"bash","arguments":arguments},
                {"type":"function_call_output","call_id":"call_id_2","output":output}
            ]
        );

        assert_eq!(value, expected);
    }

    #[test]
    fn from_api() {
        let call = FunctionToolCall {
            arguments: r#"{"command":"echo hello"}"#.into(),
            call_id: "call_id_69".into(),
            name: "bash".into(),
            id: Some("id_42".into()),
            status: None,
        };

        let ToolCallItem {
            id,
            call_id,
            task,
            timing,
            executed_at_ms,
        } = ToolCallItem::try_from(call).unwrap();

        assert_eq!(id, Some("id_42".into()));
        assert_eq!(call_id, "call_id_69");
        assert!(timing.started_at_ms > 0);
        assert_eq!(timing.last_modified_ms, None);
        assert_eq!(executed_at_ms, None);

        assert_eq!(task.typetag_name(), "bash");
        assert_eq!(task.output(), None);
        let bash_args: BashArguments = serde_json::from_str(&task.arguments()).unwrap();
        assert_eq!(bash_args.command, "echo hello");
    }
}

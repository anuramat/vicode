use async_openai::types::responses;
use serde_json::Value;

use crate::agent::tool::traits::ToolCallSerializable;
use crate::llm::message::ToolCallItem;

impl From<&ToolCallItem> for Vec<responses::InputItem> {
    fn from(v: &ToolCallItem) -> Self {
        let mut result = Vec::new();
        result.push(responses::InputItem::Item(responses::Item::FunctionCall(
            responses::FunctionToolCall {
                arguments: v.task.arguments(),
                call_id: v.call_id.clone(),
                name: v.task.typetag_name().to_string(),
                id: v.id.clone(),
                status: None,
            },
        )));
        if let Some(output) = v.task.output() {
            result.push(responses::InputItem::Item(
                responses::Item::FunctionCallOutput(responses::FunctionCallOutputItemParam {
                    call_id: v.call_id.clone(),
                    id: None,
                    output: responses::FunctionCallOutput::Text(output),
                    status: None,
                }),
            ));
        }
        result
    }
}

impl TryFrom<responses::FunctionToolCall> for ToolCallItem {
    type Error = anyhow::Error;

    fn try_from(call: responses::FunctionToolCall) -> Result<Self, Self::Error> {
        let temp = serde_json::json!({
            "name": call.name,
            "arguments": serde_json::from_str::<Value>(&call.arguments)?,
        });
        let task = serde_json::from_value::<Box<dyn ToolCallSerializable>>(temp)?;
        Ok(ToolCallItem {
            id: call.id,
            call_id: call.call_id,
            task,
        })
    }
}

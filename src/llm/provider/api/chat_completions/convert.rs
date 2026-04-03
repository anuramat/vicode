use anyhow::Result;
use async_openai::types::chat::ChatCompletionMessageToolCalls;
use async_openai::types::chat::ChatCompletionTool;
use async_openai::types::chat::ChatCompletionTools;
use async_openai::types::chat::FunctionObject;
use serde_json::Value;

use crate::agent::tool::registry::ToolSchemas;
use crate::agent::tool::traits::ToolCallSerializable;
use crate::llm::message::ItemTiming;
use crate::llm::message::ToolCallItem;

impl From<ToolSchemas> for Vec<ChatCompletionTools> {
    fn from(schema: ToolSchemas) -> Self {
        schema
            .0
            .into_iter()
            .map(|tool| {
                ChatCompletionTools::Function(ChatCompletionTool {
                    function: FunctionObject {
                        name: tool.name,
                        description: Some(tool.description),
                        parameters: Some(tool.parameters),
                        strict: Some(true),
                    },
                })
            })
            .collect()
    }
}

impl TryFrom<ChatCompletionMessageToolCalls> for ToolCallItem {
    type Error = anyhow::Error;

    fn try_from(call: ChatCompletionMessageToolCalls) -> Result<Self, Self::Error> {
        match call {
            ChatCompletionMessageToolCalls::Function(call) => {
                let temp = serde_json::json!({
                    "name": call.function.name,
                    "arguments": serde_json::from_str::<Value>(&call.function.arguments)?,
                });
                let task = serde_json::from_value::<Box<dyn ToolCallSerializable>>(temp)?;
                Ok(ToolCallItem {
                    id: Some(call.id.clone()),
                    call_id: call.id,
                    timing: ItemTiming::new(),
                    executed_at_ms: None,
                    task,
                })
            }
            ChatCompletionMessageToolCalls::Custom(_) => {
                anyhow::bail!("custom chat tools are unsupported")
            }
        }
    }
}

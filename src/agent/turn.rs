use anyhow::Result;
use futures::StreamExt;
use tracing::instrument;
use tracing::trace;

use super::Agent;
use super::Assistant;
use crate::agent::task::sink::TurnHandle;
use crate::agent::tool::registry::ToolRegistry;
use crate::llm::history::AssistantEvent;
use crate::llm::history::message::Message;

// TODO should these ResponseFailed events also coincide with ParentEvent::Error? and if so, should we emit ParentEvent::Error right here or in the HistoryEvent handler in the agent event loop?

impl Agent {
    /// pump one assistant turn from the provider stream into the task sink
    #[instrument(skip(handle, instructions, messages, assistant, tools))]
    pub async fn turn(
        handle: TurnHandle,
        assistant: &Assistant,
        tools: ToolRegistry,
        instructions: String,
        messages: Vec<Message>,
    ) -> Result<()> {
        let started = assistant.stream_turn(instructions, messages, tools).await?;
        handle
            .send(AssistantEvent::Started {
                started_at: started.started_at,
            })
            .await?;
        let mut stream = started.stream;
        while let Some(event) = stream.next().await {
            trace!(event = ?event, "Stream chunk received");
            handle.send(event?).await?;
        }
        Ok(())
    }
}

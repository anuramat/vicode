use anyhow::Result;
use futures::TryStreamExt;
use tracing::instrument;
use tracing::trace;

use super::Agent;
use super::Assistant;
use crate::agent::task::sink::TaskSink;
use crate::agent::tool::registry::ToolRegistry;
use crate::llm::history::Summary;
use crate::llm::history::message::CompactMessage;
use crate::llm::history::message::Message;
use crate::utils::now;

impl Agent {
    #[instrument(skip(sink, instructions, messages, assistant, tools))]
    pub async fn turn(
        sink: TaskSink,
        assistant: &Assistant,
        tools: ToolRegistry,
        instructions: String,
        messages: Vec<Message>,
    ) -> Result<()> {
        let mut stream = assistant.stream_turn(instructions, messages, tools).await?;
        while let Some(event) = stream.try_next().await? {
            trace!(event = ?event, "Stream chunk received");
            sink.stream(event)?;
        }
        Ok(())
    }

    #[instrument(skip(instructions, messages, assistant))]
    pub async fn summarize(
        assistant: &Assistant,
        instructions: String,
        messages: Vec<Message>,
    ) -> Result<CompactMessage> {
        let mut summary = Summary::new(now());
        let mut stream = assistant
            .stream_turn(instructions, messages, ToolRegistry::empty())
            .await?;
        while let Some(event) = stream.try_next().await? {
            summary.handle(event)?;
        }
        summary.finish()
    }
}

use anyhow::Result;
use futures::TryStreamExt;
use futures::future::ready;
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

// TODO should these ResponseFailed events also coincide with UiEvent::Error? and if so, should we emit UiEvent::Error right here or in the HistoryEvent handler in the agent event loop?

impl Agent {
    /// pump one assistant turn from the provider stream into the task sink
    #[instrument(skip(sink, instructions, messages, assistant, tools))]
    pub async fn turn(
        sink: TaskSink,
        assistant: &Assistant,
        tools: ToolRegistry,
        instructions: String,
        messages: Vec<Message>,
    ) -> Result<()> {
        let stream = assistant.stream_turn(instructions, messages, tools).await?;
        stream
            .try_for_each(|event| {
                trace!(event = ?event, "Stream chunk received");
                ready(sink.stream(event))
            })
            .await
    }

    /// run a tool-less request to completion; its text output is the summary
    #[instrument(skip(instructions, messages, assistant))]
    pub async fn summarize(
        assistant: &Assistant,
        instructions: String,
        messages: Vec<Message>,
    ) -> Result<CompactMessage> {
        let mut summary = Summary::new(now());
        let stream = assistant
            .stream_turn(instructions, messages, ToolRegistry::empty())
            .await?;
        stream
            .try_for_each(|event| ready(summary.handle(event)))
            .await?;
        summary.finish()
    }
}

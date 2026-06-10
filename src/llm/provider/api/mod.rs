use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt;
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::tool::registry::ToolRegistry;
use crate::config::ModelConfig;
use crate::llm::history::AssistantEvent;
use crate::llm::history::message::Message;

/// SLOP `chat_completions` module is vibecoded
#[allow(deprecated, clippy::pedantic, clippy::nursery, clippy::style)]
pub mod chat_completions;
/// SLOP `chatgpt` module is vibecoded
#[allow(deprecated, clippy::pedantic, clippy::nursery, clippy::style)]
pub mod chatgpt;
pub mod responses;

pub type AssistantStream =
    Pin<Box<dyn Stream<Item = Result<AssistantEvent, anyhow::Error>> + Send>>;

pub struct StartedAssistantStream {
    pub started_at: u64,
    pub stream: AssistantStream,
}

/// ends the stream after `Completed`: the inner stream is dropped immediately
/// (releasing its permit) and never polled past completion
pub fn until_completed(stream: AssistantStream) -> AssistantStream {
    Box::pin(futures::stream::unfold(Some(stream), |state| async {
        let mut stream = state?;
        let event = stream.next().await?;
        let done = matches!(event, Ok(AssistantEvent::Completed { .. }));
        Some((event, (!done).then_some(stream)))
    }))
}

// TODO would these work with references? specifically the messages
#[async_trait]
pub trait Api: Send + Sync + std::fmt::Debug {
    async fn stream(
        &self,
        permit: OwnedSemaphorePermit,
        // TODO can we somehow avoid model config here? ideally don't pass it around
        model: ModelConfig,
        instructions: String,
        messages: Vec<Message>,
        tools: ToolRegistry,
    ) -> Result<StartedAssistantStream>;
}

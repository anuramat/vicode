use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
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
    pub started_at_ms: u64,
    pub stream: AssistantStream,
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

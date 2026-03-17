use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::tool::registry::ToolSchemas;
use crate::config::AssistantModelConfig;
use crate::llm::api::event::StreamEvent;
use crate::llm::history::History;

pub mod chat_completions;
pub mod responses;

pub type AssistantStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, anyhow::Error>> + Send>>;

#[async_trait]
pub trait Backend: Send + Sync {
    async fn stream(
        &self,
        permit: OwnedSemaphorePermit,
        // TODO can we somehow avoid model config here? ideally don't pass it around
        model: AssistantModelConfig,
        instructions: String,
        history: History,
        tools: ToolSchemas,
    ) -> Result<AssistantStream>;
}

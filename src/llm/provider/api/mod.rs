use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::tool::registry::ToolSchemas;
use crate::config::ModelConfig;
use crate::llm::delta::Delta;
use crate::llm::message::AssistantItem;
use crate::llm::message::Message;

pub mod chat_completions;
pub mod responses;

pub type AssistantStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, anyhow::Error>> + Send>>;

pub struct StartedAssistantStream {
    pub started_at_ms: u64,
    pub stream: AssistantStream,
}

#[async_trait]
pub trait Api: Send + Sync + std::fmt::Debug {
    async fn stream(
        &self,
        permit: OwnedSemaphorePermit,
        // TODO can we somehow avoid model config here? ideally don't pass it around
        model: ModelConfig,
        instructions: String,
        messages: Vec<Message>,
        tools: ToolSchemas,
    ) -> Result<StartedAssistantStream>;
}

#[derive(Debug)]
pub enum StreamEvent {
    Delta(Delta),
    ItemDone(AssistantItem),
    ItemAdded(AssistantItem),
    Failed(String),
    Completed(Vec<AssistantItem>),
    Ignore,
}

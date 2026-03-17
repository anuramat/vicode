/// WARN this entire module is vibecoded
use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_trait::async_trait;
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::tool::registry::ToolSchemas;
use crate::config::ApiCompatConfig;
use crate::config::ApiConfig;
use crate::config::AssistantModelConfig;
use crate::llm::api::backend::AssistantStream;
use crate::llm::api::backend::Backend;
use crate::llm::history::History;

mod convert;
mod request;
mod stream;

pub struct ChatCompletionsBackend {
    client: Client<OpenAIConfig>,
    compat: ApiCompatConfig,
    reasoning_key: Option<String>,
}

impl ChatCompletionsBackend {
    pub fn new(
        client: Client<OpenAIConfig>,
        config: ApiConfig,
    ) -> Self {
        Self {
            client,
            compat: config.compat,
            reasoning_key: config.reasoning_key,
        }
    }
}

#[async_trait]
impl Backend for ChatCompletionsBackend {
    async fn stream(
        &self,
        permit: OwnedSemaphorePermit,
        config: AssistantModelConfig,
        instructions: String,
        history: History,
        tools: ToolSchemas,
    ) -> Result<AssistantStream> {
        let request = request::request(
            config,
            instructions,
            history,
            tools,
            true,
            &self.compat,
            self.reasoning_key.as_deref(),
        )?;
        tracing::debug!(request = %request);
        let inner = self.client.chat().create_stream_byot(request).await?;
        Ok(Box::pin(stream::ChatCompletionsStream::new(
            inner,
            permit,
            self.reasoning_key.clone(),
        )))
    }
}

/// WARN this entire module is vibecoded
use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_trait::async_trait;
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::tool::registry::ToolSchemas;
use crate::config::ApiCompatConfig;
use crate::config::ModelConfig;
use crate::config::ProviderConfig;
use crate::llm::history::History;
use crate::llm::provider::api::Api;
use crate::llm::provider::api::AssistantStream;

mod convert;
mod request;
mod stream;

pub struct ChatCompletionsApi {
    client: Client<OpenAIConfig>,
    compat: ApiCompatConfig,
    reasoning_key: Option<String>,
}

impl ChatCompletionsApi {
    pub fn new(
        client: Client<OpenAIConfig>,
        config: ProviderConfig,
    ) -> Self {
        Self {
            client,
            compat: config.compat,
            reasoning_key: config.reasoning_key,
        }
    }
}

#[async_trait]
impl Api for ChatCompletionsApi {
    async fn stream(
        &self,
        permit: OwnedSemaphorePermit,
        config: ModelConfig,
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

/// SLOP entire chat completions module is vibecoded
use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_trait::async_trait;
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::tool::registry::ToolSchemas;
use crate::config::ApiCompatConfig;
use crate::config::ModelConfig;
use crate::config::ProviderConfig;
use crate::llm::message::Message;
use crate::llm::message::now_ms;
use crate::llm::provider::api::Api;
use crate::llm::provider::api::StartedAssistantStream;

mod convert;
mod request;
mod stream;

#[derive(Debug)]
pub struct ChatCompletionsApi {
    client: Client<OpenAIConfig>,
    compat: ApiCompatConfig,
}

impl ChatCompletionsApi {
    pub fn new(
        client: Client<OpenAIConfig>,
        config: ProviderConfig,
    ) -> Self {
        Self {
            client,
            compat: config.compat,
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
        history: Vec<Message>,
        tools: ToolSchemas,
    ) -> Result<StartedAssistantStream> {
        let request = request::request(config, instructions, history, tools, true, &self.compat)?;
        tracing::debug!(request = %request);
        let started_at_ms = now_ms();
        let inner = self.client.chat().create_stream_byot(request).await?;
        Ok(StartedAssistantStream {
            started_at_ms,
            stream: Box::pin(stream::ChatCompletionsStream::new(
                inner,
                permit,
                self.compat.reasoning_content_field.clone(),
            )),
        })
    }
}

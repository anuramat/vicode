mod convert;
mod request;
mod stream;

use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_trait::async_trait;
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::tool::registry::ToolRegistry;
use crate::config::ApiCompatConfig;
use crate::config::ModelConfig;
use crate::llm::history::message::Message;
use crate::llm::provider::api::Api;
use crate::llm::provider::api::StartedAssistantStream;
use crate::utils::now;

#[derive(Debug)]
pub struct ChatCompletionsApi {
    client: Client<OpenAIConfig>,
    compat: ApiCompatConfig,
}

impl ChatCompletionsApi {
    pub fn new(
        client: Client<OpenAIConfig>,
        compat: ApiCompatConfig,
    ) -> Self {
        Self { client, compat }
    }
}

#[async_trait]
impl Api for ChatCompletionsApi {
    async fn stream(
        &self,
        permit: OwnedSemaphorePermit,
        model: ModelConfig,
        instructions: String,
        messages: Vec<Message>,
        tools: ToolRegistry,
    ) -> Result<StartedAssistantStream> {
        let request = request::request(model, instructions, messages, tools, true, &self.compat)?;
        tracing::debug!(request = %request);
        let started_at_ms = now();
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

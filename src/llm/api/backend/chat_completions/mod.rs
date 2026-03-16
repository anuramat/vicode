/// WARN this entire module is vibecoded
use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::CreateChatCompletionRequestArgs;
use async_trait::async_trait;
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::tool::registry::ToolSchemas;
use crate::config::ApiCompatConfig;
use crate::config::CONFIG;
use crate::llm::api::backend::AssistantStream;
use crate::llm::api::backend::Backend;
use crate::llm::history::History;

mod convert;
mod request;
mod stream;

pub struct ChatCompletionsBackend {
    client: Client<OpenAIConfig>,
    compat: ApiCompatConfig,
    request_builder: CreateChatCompletionRequestArgs,
}

impl ChatCompletionsBackend {
    pub fn new(
        client: Client<OpenAIConfig>,
        compat: ApiCompatConfig,
    ) -> Self {
        let mut request_builder = CreateChatCompletionRequestArgs::default();
        request_builder
            .model(&CONFIG.api.model_name)
            .parallel_tool_calls(true);
        if let Some(effort) = CONFIG.api.effort.clone() {
            request_builder.reasoning_effort(effort);
        }
        Self {
            client,
            compat,
            request_builder,
        }
    }
}

#[async_trait]
impl Backend for ChatCompletionsBackend {
    async fn stream(
        &self,
        permit: OwnedSemaphorePermit,
        instructions: String,
        history: History,
        tools: ToolSchemas,
    ) -> Result<AssistantStream> {
        let request = request::request(
            self.request_builder.clone(),
            instructions,
            history,
            tools,
            true,
            &self.compat,
            CONFIG.api.reasoning_key.as_deref(),
        )?;
        tracing::debug!(request = %request);
        let inner = self.client.chat().create_stream_byot(request).await?;
        Ok(Box::pin(stream::ChatCompletionsStream::new(
            inner,
            permit,
            CONFIG.api.reasoning_key.clone(),
        )))
    }
}

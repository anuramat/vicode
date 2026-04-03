pub mod api;
pub mod assistant;
pub mod compat;
pub mod request;

use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use governor::DefaultDirectRateLimiter;
use governor::Quota;
use governor::RateLimiter;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use smart_default::SmartDefault;
use tokio::sync::Semaphore;

use crate::llm::provider::api::Api;
use crate::llm::provider::api::chat_completions::ChatCompletionsApi;
use crate::llm::provider::api::responses::ResponsesApi;

#[derive(Debug)]
pub struct Provider {
    pub config: ProviderConfig,
    pub ratelimiter: DefaultDirectRateLimiter,
    pub api: Arc<dyn Api>,
    pub semaphore: Arc<Semaphore>,
}

#[derive(Deserialize, Debug, Clone, SmartDefault, Serialize, JsonSchema)]
#[serde(default)]
pub struct ProviderConfig {
    pub api: ApiType,
    /// base URL for the api; expands env vars
    #[default = "localhost"]
    pub base_url: String,
    /// bash command that outputs the key to stdout
    pub key_command: Option<String>,

    /// compatibility hacks
    #[serde(flatten)]
    pub compat: ApiCompatConfig,

    /// max number of concurrent requests
    #[default = 1]
    pub concurrency: usize,
    /// max requests per minute
    #[default = 60]
    pub rpm: u32,
    /// max retries
    #[default = 5]
    pub retries: usize,
    /// initial retry delay, multiplied by 2 after each attempt
    #[default = 1000]
    pub backoff_ms: u64,
}

#[derive(Deserialize, Default, Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApiType {
    #[default]
    Responses,
    ChatCompletions,
}

#[derive(Deserialize, Debug, Clone, SmartDefault, Serialize, JsonSchema)]
#[serde(default)]
pub struct ApiCompatConfig {
    /// name of the field with reasoning content (chat completions only)
    #[default = "reasoning_content"]
    pub reasoning_content_field: String,
    pub instructions_as_message: bool,
    pub reasoning_as_output: Option<String>,
    pub developer_as_user: bool,
}

impl ProviderConfig {
    pub fn base_url(&self) -> Result<String> {
        Ok(shellexpand::full(&self.base_url)?.into_owned())
    }
}

impl Provider {
    async fn new(provider_config: ProviderConfig) -> Result<Self> {
        let openai_config = {
            let mut openai_config = async_openai::config::OpenAIConfig::new()
                .with_api_base(&provider_config.base_url()?);
            if let Some(key) = Self::key(provider_config.key_command.as_deref()).await? {
                openai_config = openai_config.with_api_key(key);
            }
            openai_config
        };

        let client = async_openai::Client::with_config(openai_config);
        let api: Arc<dyn Api> = match provider_config.api {
            crate::config::ApiType::Responses => {
                Arc::new(ResponsesApi::new(client, provider_config.clone()))
            }
            crate::config::ApiType::ChatCompletions => {
                Arc::new(ChatCompletionsApi::new(client, provider_config.clone()))
            }
        };

        Ok(Self {
            ratelimiter: RateLimiter::direct(Quota::per_minute(
                provider_config
                    .rpm
                    .try_into()
                    .with_context(|| "invalid rpm provided")?,
            )),
            api,
            semaphore: Arc::new(Semaphore::new(provider_config.concurrency)),
            config: provider_config,
        })
    }

    async fn key(command: Option<&str>) -> Result<Option<String>> {
        let Some(command) = command else {
            return Ok(None);
        };
        let output = tokio::process::Command::new("bash")
            .args(["-c", command])
            .output()
            .await
            .context("Failed to run API key command")?;
        anyhow::ensure!(
            output.status.success(),
            "API key command failed with status {}",
            output.status
        );
        Ok(Some(
            String::from_utf8(output.stdout)
                .context("API key command did not produce valid UTF-8")?
                .trim()
                .to_string(),
        ))
    }
}

pub mod api;
pub mod assistant;
pub mod compat;
pub mod request;

use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use governor::DefaultDirectRateLimiter;
use governor::Quota;
use governor::RateLimiter;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use smart_default::SmartDefault;
use tokio::sync::Semaphore;

use crate::deps;
use crate::llm::provider::api::Api;
use crate::llm::provider::api::chat_completions::ChatCompletionsApi;
use crate::llm::provider::api::chatgpt::ChatgptApi;
use crate::llm::provider::api::chatgpt::ChatgptAuthManager;
use crate::llm::provider::api::responses::ResponsesApi;

#[derive(Debug)]
pub struct Provider {
    pub config: ProviderConfig,
    pub ratelimiter: DefaultDirectRateLimiter,
    pub api: Arc<dyn Api>,
    pub semaphore: Arc<Semaphore>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "api", rename_all = "snake_case")]
pub enum ProviderConfig {
    Responses(ApiKeyProvider),
    ChatCompletions(ApiKeyProvider),
    Chatgpt(ChatgptProvider),
}

#[derive(Deserialize, Debug, Clone, SmartDefault, Serialize, JsonSchema)]
#[serde(default)]
pub struct ApiKeyProvider {
    /// base URL for the api; expands env vars
    #[default = "localhost"]
    pub base_url: String,
    /// bash command that outputs the key to stdout
    pub key_command: Option<String>,

    /// compatibility hacks
    #[serde(flatten)]
    pub compat: ApiCompatConfig,

    #[serde(flatten)]
    pub limits: RateLimits,
}

#[derive(Deserialize, Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ChatgptProvider {
    #[serde(flatten)]
    pub limits: RateLimits,
}

#[derive(Deserialize, Debug, Clone, SmartDefault, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RateLimits {
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
    pub fn limits(&self) -> &RateLimits {
        match self {
            Self::Responses(p) | Self::ChatCompletions(p) => &p.limits,
            Self::Chatgpt(p) => &p.limits,
        }
    }

    pub fn is_chatgpt(&self) -> bool {
        matches!(self, Self::Chatgpt(_))
    }

    /// runs `key_command` (if any) to resolve the API key
    pub async fn resolve_key(&self) -> Result<Option<String>> {
        match self {
            Self::Responses(p) | Self::ChatCompletions(p) => key(p.key_command.as_deref()).await,
            Self::Chatgpt(_) => Ok(None),
        }
    }
}

impl Provider {
    fn new(
        provider_id: String,
        provider_config: ProviderConfig,
        key: Option<String>,
    ) -> Result<Self> {
        let api: Arc<dyn Api> = match &provider_config {
            ProviderConfig::Responses(p) => {
                Arc::new(ResponsesApi::new(openai_client(p, key)?, p.compat.clone()))
            }
            ProviderConfig::ChatCompletions(p) => Arc::new(ChatCompletionsApi::new(
                openai_client(p, key)?,
                p.compat.clone(),
            )),
            ProviderConfig::Chatgpt(_) => {
                Arc::new(ChatgptApi::new(ChatgptAuthManager::new(&provider_id)?))
            }
        };

        let limits = provider_config.limits();
        Ok(Self {
            ratelimiter: RateLimiter::direct(Quota::per_minute(
                limits
                    .rpm
                    .try_into()
                    .with_context(|| "invalid rpm provided")?,
            )),
            api,
            semaphore: Arc::new(Semaphore::new(limits.concurrency)),
            config: provider_config,
        })
    }
}

fn openai_client(
    p: &ApiKeyProvider,
    key: Option<String>,
) -> Result<Client<OpenAIConfig>> {
    let base = shellexpand::full(&p.base_url)?;
    let mut cfg = OpenAIConfig::new().with_api_base(&*base);
    if let Some(k) = key {
        cfg = cfg.with_api_key(k);
    }
    Ok(Client::with_config(cfg))
}

async fn key(command: Option<&str>) -> Result<Option<String>> {
    let Some(command) = command else {
        return Ok(None);
    };
    let output = tokio::process::Command::new(deps::BASH)
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

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;

    fn api_key_config(key_command: Option<&str>) -> ProviderConfig {
        ProviderConfig::Responses(ApiKeyProvider {
            key_command: key_command.map(Into::into),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn resolve_key_execs_and_trims() {
        let key = api_key_config(Some("echo ' secret '"))
            .resolve_key()
            .await
            .unwrap();
        assert_eq!(key, Some("secret".to_string()));
    }

    #[tokio::test]
    async fn resolve_key_errors_on_failing_command() {
        assert!(api_key_config(Some("false")).resolve_key().await.is_err());
    }

    #[tokio::test]
    async fn resolve_key_none_without_command() {
        assert_eq!(api_key_config(None).resolve_key().await.unwrap(), None);
        let chatgpt = ProviderConfig::Chatgpt(ChatgptProvider::default());
        assert_eq!(chatgpt.resolve_key().await.unwrap(), None);
    }
}

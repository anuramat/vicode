use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use governor::DefaultDirectRateLimiter;
use governor::Quota;
use governor::RateLimiter;
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::config::CONFIG;
use crate::llm::api::backend;
use crate::llm::api::backend::Backend;

pub struct Assistant {
    pub ratelimiter: DefaultDirectRateLimiter,
    pub backend: Arc<dyn Backend>,
    pub semaphore: Arc<Semaphore>,
}

impl Assistant {
    pub async fn new() -> Result<Self> {
        let openai_config = {
            let mut config = OpenAIConfig::new().with_api_base(&CONFIG.api.base_url()?);
            if let Some(key) = Self::key().await? {
                config = config.with_api_key(key);
            }
            config
        };

        let client = Client::with_config(openai_config);
        let compat = CONFIG.api.compat.clone();

        let backend: Arc<dyn Backend> = match CONFIG.api.kind {
            crate::config::ApiKind::Responses => {
                Arc::new(backend::responses::ResponsesBackend::new(client, compat))
            }
            crate::config::ApiKind::ChatCompletions => Arc::new(
                backend::chat_completions::ChatCompletionsBackend::new(client, compat),
            ),
        };

        let semaphore = Arc::new(Semaphore::new(CONFIG.api.concurrency));
        let ratelimiter =
            RateLimiter::direct(Quota::per_second(CONFIG.api.rps.try_into().unwrap()));

        Ok(Self {
            ratelimiter,
            backend,
            semaphore,
        })
    }

    pub async fn key() -> Result<Option<String>> {
        let Some(command) = &CONFIG.api.key_command else {
            return Ok(None);
        };
        Ok(Some(Self::key_from_command(command).await?))
    }

    async fn key_from_command(command: &str) -> Result<String> {
        let output = Command::new("bash")
            .args(["-lc", command])
            .output()
            .await
            .context("Failed to run API key command")?;
        anyhow::ensure!(
            output.status.success(),
            "API key command failed with status {}",
            output.status
        );
        let key = String::from_utf8(output.stdout)
            .context("API key command did not produce valid UTF-8")?
            .trim()
            .to_string();
        Ok(key)
    }
}

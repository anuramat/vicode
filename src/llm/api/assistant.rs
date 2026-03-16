use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use governor::DefaultDirectRateLimiter;
use governor::Quota;
use governor::RateLimiter;
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
            let mut config = OpenAIConfig::new().with_api_base(&CONFIG.api.base_url);
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
        let path = if let Some(path) = &CONFIG.api.key_path {
            shellexpand::tilde(path).into_owned()
        } else {
            return Ok(None);
        };
        let key = tokio::fs::read_to_string(&path)
            .await
            .context("Failed to read API key file")?
            .trim()
            .to_string();
        Ok(Some(key))
    }
}

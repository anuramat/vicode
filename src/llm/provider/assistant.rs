use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use anyhow::Context;
use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use futures::future::try_join_all;
use governor::DefaultDirectRateLimiter;
use governor::Quota;
use governor::RateLimiter;
use tokio::sync::OnceCell;
use tokio::sync::Semaphore;

use crate::config::ProviderConfig;
use crate::config::AssistantConfig;
use crate::config::CONFIG;
use crate::config::Config;
use crate::config::SubagentAssistantConfig;
use crate::llm::provider::api::Api;
use crate::llm::provider::api::chat_completions::ChatCompletionsApi;
use crate::llm::provider::api::responses::ResponsesApi;

// TODO .get().unwrap() is kinda ugly; maybe wrap in helper functions? should we keep unwrapping or do proper error handling?
pub static ASSISTANT_POOL: OnceCell<AssistantPool> = OnceCell::const_new();

pub struct Provider {
    pub config: ProviderConfig,
    pub ratelimiter: DefaultDirectRateLimiter,
    pub api: Arc<dyn Api>,
    pub semaphore: Arc<Semaphore>,
}

pub struct Assistant {
    pub provider: Arc<Provider>,
    pub config: AssistantConfig,
}

pub struct AssistantPool {
    assistants: HashMap<String, Arc<Assistant>>,
    primary: RoundRobin,
    subagent: SubagentSelector,
}

struct RoundRobin {
    ids: Vec<String>,
    next: AtomicUsize,
}

enum SubagentSelector {
    Inherit,
    RoundRobin(RoundRobin),
}

impl Provider {
    async fn new(provider_config: ProviderConfig) -> Result<Self> {
        let openai_config = {
            let mut openai_config =
                OpenAIConfig::new().with_api_base(&provider_config.base_url()?);
            if let Some(key) = Self::key(provider_config.key_command.as_deref()).await? {
                openai_config = openai_config.with_api_key(key);
            }
            openai_config
        };

        let client = Client::with_config(openai_config);
        let api: Arc<dyn Api> = match provider_config.api {
            crate::config::ApiType::Responses => {
                Arc::new(ResponsesApi::new(client, provider_config.clone()))
            }
            crate::config::ApiType::ChatCompletions => {
                Arc::new(ChatCompletionsApi::new(client, provider_config.clone()))
            }
        };

        Ok(Self {
            ratelimiter: RateLimiter::direct(Quota::per_second(
                provider_config
                    .rps
                    .try_into()
                    .with_context(|| "invalid rps provided")?,
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
            .args(["-lc", command])
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

impl AssistantPool {
    pub async fn new() -> Result<Self> {
        Self::from_config(&CONFIG).await
    }

    async fn from_config(config: &Config) -> Result<Self> {
        let providers: HashMap<_, _> =
            {
                let futures = config.providers.iter().map(
                    async |(id, config)| -> Result<(String, Arc<Provider>)> {
                        Ok((id.clone(), Arc::new(Provider::new(config.clone()).await?)))
                    },
                );
                try_join_all(futures).await?.into_iter().collect()
            };

        let assistants: HashMap<_, _> = config
            .assistants
            .iter()
            .map(|(id, config)| {
                Ok((
                    id.clone(),
                    Arc::new(Assistant {
                        provider: providers
                            .get(&config.provider)
                            .cloned()
                            .with_context(|| format!("unknown provider {:?}", config.provider))?,
                        config: config.clone(),
                    }),
                ))
            })
            .collect::<Result<_>>()?;

        Ok(Self {
            assistants,
            primary: RoundRobin::new(config.primary_assistant.clone()),
            subagent: match &config.subagent_assistant {
                SubagentAssistantConfig::Inherit => SubagentSelector::Inherit,
                SubagentAssistantConfig::Assistants(ids) => {
                    SubagentSelector::RoundRobin(RoundRobin::new(ids.clone()))
                }
            },
        })
    }

    pub fn assistant(
        &self,
        id: &str,
    ) -> Result<Arc<Assistant>> {
        self.assistants
            .get(id)
            .cloned()
            .with_context(|| format!("unknown assistant {id:?}"))
    }

    pub fn next_primary(&self) -> String {
        self.primary.next()
    }

    pub fn next_subagent(
        &self,
        parent: &str,
    ) -> String {
        match &self.subagent {
            SubagentSelector::Inherit => parent.to_string(),
            SubagentSelector::RoundRobin(selector) => selector.next(),
        }
    }
}

impl RoundRobin {
    fn new(ids: Vec<String>) -> Self {
        Self {
            ids,
            next: AtomicUsize::new(0),
        }
    }

    fn next(&self) -> String {
        let idx = self.next.fetch_add(1, Ordering::Relaxed);
        self.ids[idx % self.ids.len()].clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::Config;

    #[test]
    fn selector_round_robins() {
        let selector = RoundRobin::new(vec!["a".into(), "b".into()]);
        assert_eq!(selector.next(), "a");
        assert_eq!(selector.next(), "b");
        assert_eq!(selector.next(), "a");
    }

    #[tokio::test]
    async fn assistants_share_provider() {
        let config = Config::parse(
            r#"
            primary_assistant = ["fast", "deep"]

            [providers.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rps = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            provider = "main"
            model = "gpt-fast"

            [assistants.deep]
            provider = "main"
            model = "gpt-deep"
            effort = "low"

            [bash]
            "#,
        )
        .unwrap();
        let pool = AssistantPool::from_config(&config).await.unwrap();
        let fast = pool.assistant("fast").unwrap();
        let deep = pool.assistant("deep").unwrap();
        assert!(Arc::ptr_eq(&fast.provider, &deep.provider));
        assert_eq!(pool.next_subagent("fast"), "fast");
    }

    #[tokio::test]
    async fn subagents_round_robin_over_subset() {
        let config = Config::parse(
            r#"
            primary_assistant = ["fast"]
            subagent_assistant = ["deep", "fast"]

            [providers.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rps = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            provider = "main"
            model = "gpt-fast"

            [assistants.deep]
            provider = "main"
            model = "gpt-deep"

            [bash]
            "#,
        )
        .unwrap();
        let pool = AssistantPool::from_config(&config).await.unwrap();
        assert_eq!(pool.next_subagent("fast"), "deep");
        assert_eq!(pool.next_subagent("fast"), "fast");
    }
}

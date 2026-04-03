use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use anyhow::Context;
use anyhow::Result;
use futures::future::try_join_all;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_plain::derive_deserialize_from_fromstr;
use serde_plain::derive_serialize_from_display;
use tokio::sync::OnceCell;

use super::Provider;
use crate::config::CONFIG;
use crate::config::Config;

// TODO .get().unwrap() is kinda ugly; maybe wrap in helper functions? should we keep unwrapping or do proper error handling?
pub static ASSISTANT_POOL: OnceCell<AssistantPool> = OnceCell::const_new();

#[derive(Debug, Clone)]
pub struct Assistant {
    pub id: String,
    pub provider: Arc<Provider>,
    pub config: ModelConfig,
}

derive_serialize_from_display!(Assistant);
impl std::fmt::Display for Assistant {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

derive_deserialize_from_fromstr!(Assistant, "existing assistant id");
impl std::str::FromStr for Assistant {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ASSISTANT_POOL
            .get()
            .context("assistant pool not initialized")?
            .assistant(s)
    }
}

#[derive(Deserialize, Debug, Clone, Serialize, JsonSchema)]
pub struct AssistantConfig {
    pub provider: String,
    #[serde(flatten)]
    pub model: ModelConfig,
}

#[derive(Clone, Serialize, Debug, Deserialize, PartialEq, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    Xhigh,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ModelConfig {
    pub model: String,
    pub effort: Option<ReasoningEffort>,
    /// max context window
    pub window: Option<usize>,
}

pub struct AssistantPool {
    assistants: IndexMap<String, Assistant>,
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

impl AssistantPool {
    pub async fn new() -> Result<Self> {
        Self::from_config(&CONFIG).await
    }

    pub async fn from_config(config: &Config) -> Result<Self> {
        let providers: HashMap<_, _> = {
            let futures = config.providers.iter().map(
                async |(id, config)| -> Result<(String, Arc<Provider>)> {
                    Ok((id.clone(), Arc::new(Provider::new(config.clone()).await?)))
                },
            );
            try_join_all(futures).await?.into_iter().collect()
        };

        let assistants: IndexMap<_, _> = config
            .assistants
            .iter()
            .map(|(id, config)| {
                Ok((
                    id.clone(),
                    Assistant {
                        id: id.clone(),
                        provider: providers
                            .get(&config.provider)
                            .cloned()
                            .with_context(|| format!("unknown provider {:?}", config.provider))?,
                        config: config.model.clone(),
                    },
                ))
            })
            .collect::<Result<_>>()?;

        Ok(Self {
            assistants,
            primary: RoundRobin::new(config.primary_assistant.clone()),
            subagent: if config.subagent_assistant.is_empty() {
                SubagentSelector::Inherit
            } else {
                SubagentSelector::RoundRobin(RoundRobin::new(config.subagent_assistant.clone()))
            },
        })
    }

    pub fn assistant(
        &self,
        id: &str,
    ) -> Result<Assistant> {
        self.assistants
            .get(id)
            .cloned()
            .with_context(|| format!("unknown assistant {id:?}"))
    }

    pub fn next_primary(&self) -> String {
        self.primary.next()
    }

    pub fn switch_assistant(
        &self,
        id: &str,
        step: isize,
    ) -> Option<String> {
        let len = self.assistants.len();
        let step = step % len as isize;
        let old = (len + self.assistants.get_index_of(id)?) as isize;
        let new = (old + step) as usize;
        Some(self.assistants.get_index(new % len)?.0.clone())
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
            shell_cmd = ["bash", "-c"]

            [sandbox]
            kind = "bwrap"
            bin = "bwrap"
            args = []
            stages = []

            [keymap.cmdline]

            [keymap.normal]

            [keymap.insert]

            [providers.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rpm = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            provider = "main"
            model = "gpt-fast"

            [assistants.deep]
            provider = "main"
            model = "gpt-deep"
            effort = "low"

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
            shell_cmd = ["bash", "-c"]

            [sandbox]
            kind = "bwrap"
            bin = "bwrap"
            args = []
            stages = []

            [keymap.cmdline]

            [keymap.normal]

            [keymap.insert]

            [providers.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rpm = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            provider = "main"
            model = "gpt-fast"

            [assistants.deep]
            provider = "main"
            model = "gpt-deep"

            "#,
        )
        .unwrap();
        let pool = AssistantPool::from_config(&config).await.unwrap();
        assert_eq!(pool.next_subagent("fast"), "deep");
        assert_eq!(pool.next_subagent("fast"), "fast");
    }

    #[tokio::test]
    async fn switch_assistant_steps_forward_through_full_order() {
        let config = Config::parse(
            r#"
            primary_assistant = ["fast"]
            shell_cmd = ["bash", "-c"]

            [sandbox]
            kind = "bwrap"
            bin = "bwrap"
            args = []
            stages = []

            [keymap.cmdline]

            [keymap.normal]

            [keymap.insert]

            [providers.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rpm = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            provider = "main"
            model = "gpt-fast"

            [assistants.deep]
            provider = "main"
            model = "gpt-deep"

            [assistants.alt]
            provider = "main"
            model = "gpt-alt"

            "#,
        )
        .unwrap();
        let pool = AssistantPool::from_config(&config).await.unwrap();
        let ids: Vec<_> = config.assistants.keys().cloned().collect();
        for pair in ids.windows(2) {
            assert_eq!(pool.switch_assistant(&pair[0], 1).unwrap(), pair[1]);
        }
        assert_eq!(
            pool.switch_assistant(ids.last().unwrap(), 1).unwrap(),
            ids[0]
        );
    }

    #[tokio::test]
    async fn switch_assistant_steps_backward_through_full_order() {
        let config = Config::parse(
            r#"
            primary_assistant = ["fast"]
            shell_cmd = ["bash", "-c"]

            [sandbox]
            kind = "bwrap"
            bin = "bwrap"
            args = []
            stages = []

            [keymap.cmdline]

            [keymap.normal]

            [keymap.insert]

            [providers.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rpm = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            provider = "main"
            model = "gpt-fast"

            [assistants.deep]
            provider = "main"
            model = "gpt-deep"

            [assistants.alt]
            provider = "main"
            model = "gpt-alt"

            "#,
        )
        .unwrap();
        let pool = AssistantPool::from_config(&config).await.unwrap();
        let ids: Vec<_> = config.assistants.keys().cloned().collect();
        for pair in ids.windows(2) {
            assert_eq!(pool.switch_assistant(&pair[1], -1).unwrap(), pair[0]);
        }
        assert_eq!(
            pool.switch_assistant(&ids[0], -1).unwrap(),
            ids.last().unwrap().clone()
        );
    }
}

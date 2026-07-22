use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use futures::future::try_join_all;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use super::Provider;
use crate::config::Config;

#[derive(Debug, Clone)]
pub struct Assistant {
    pub id: String,
    pub provider: Arc<Provider>,
    pub config: ModelConfig,
}

#[derive(Deserialize, Debug, Clone, Serialize, JsonSchema)]
pub struct AssistantConfig {
    pub provider: String,
    #[serde(flatten)]
    pub model: ModelConfig,
}

#[derive(Clone, Serialize, Debug, Deserialize, PartialEq, Eq, Default, JsonSchema)]
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

#[derive(Debug)]
pub struct AssistantPool {
    assistants: IndexMap<String, Assistant>,
    primary: String,
    /// if unset, subagents inherit their parent's assistant
    subagent: Option<String>,
}

impl AssistantPool {
    pub async fn from_config(config: &Config) -> Result<Self> {
        let providers: HashMap<_, _> = {
            // TODO stream::iter map buffered try_collect
            let futures = config.providers.iter().map(
                async |(id, config)| -> Result<(String, Arc<Provider>)> {
                    let key = config.resolve_key().await?;
                    Ok((
                        id.clone(),
                        Arc::new(Provider::new(id.clone(), config.clone(), key)?),
                    ))
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
            primary: config.primary_assistant.clone(),
            subagent: config.subagent_assistant.clone(),
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

    pub fn primary(&self) -> Result<Assistant> {
        self.assistant(&self.primary)
    }

    pub fn switch_assistant(
        &self,
        id: &str,
        prev: bool,
    ) -> Option<String> {
        let len = self.assistants.len();
        let old = self.assistants.get_index_of(id)?;
        let new = if prev {
            old.checked_sub(1).unwrap_or(len - 1)
        } else {
            old.wrapping_add(1) % len
        };
        Some(self.assistants.get_index(new)?.0.clone())
    }

    pub fn subagent(
        &self,
        parent: &str,
    ) -> Result<Assistant> {
        self.assistant(self.subagent.as_deref().unwrap_or(parent))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use similar_asserts::assert_eq;

    use super::*;
    use crate::config::Config;

    /// snapshots render an assistant as its id
    impl serde::Serialize for Assistant {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            serializer.serialize_str(&self.id)
        }
    }

    impl AssistantPool {
        /// pool with assistants `"test"` (primary) and `"test2"` sharing one
        /// scripted `FakeApi`, so switching assistants keeps the scripted turns
        pub fn fake() -> (Self, Arc<crate::llm::provider::api::fake::FakeApi>) {
            let (assistant, api) = Assistant::fake();
            let second = Assistant {
                id: "test2".into(),
                ..assistant.clone()
            };
            let pool = Self {
                primary: assistant.id.clone(),
                assistants: IndexMap::from([
                    (assistant.id.clone(), assistant),
                    (second.id.clone(), second),
                ]),
                subagent: None,
            };
            (pool, api)
        }
    }

    #[tokio::test]
    async fn assistants_share_provider() {
        let config = Config::parse_with_defaults(
            r#"
            primary_assistant = "fast"
            shell_cmd = ["bash", "-c"]

            [sandbox]
            kind = "bwrap"
            bin = "bwrap"
            args = []
            stages = []

            [providers.main]
            api = "responses"
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
        assert_eq!(pool.primary().unwrap().id, "fast");
        assert_eq!(pool.subagent("fast").unwrap().id, "fast");
    }

    #[tokio::test]
    async fn subagents_use_configured_assistant() {
        let config = Config::parse_with_defaults(
            r#"
            primary_assistant = "fast"
            subagent_assistant = "deep"
            shell_cmd = ["bash", "-c"]

            [sandbox]
            kind = "bwrap"
            bin = "bwrap"
            args = []
            stages = []

            [providers.main]
            api = "responses"
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
        assert_eq!(pool.subagent("fast").unwrap().id, "deep");
    }

    #[tokio::test]
    async fn switch_assistant_steps_forward_through_full_order() {
        let config = Config::parse_with_defaults(
            r#"
            primary_assistant = "fast"
            shell_cmd = ["bash", "-c"]

            [sandbox]
            kind = "bwrap"
            bin = "bwrap"
            args = []
            stages = []

            [providers.main]
            api = "responses"
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
            assert_eq!(pool.switch_assistant(&pair[0], false).unwrap(), pair[1]);
        }
        assert_eq!(
            pool.switch_assistant(ids.last().unwrap(), false).unwrap(),
            ids[0]
        );
    }

    #[tokio::test]
    async fn switch_assistant_steps_backward_through_full_order() {
        let config = Config::parse_with_defaults(
            r#"
            primary_assistant = "fast"
            shell_cmd = ["bash", "-c"]

            [sandbox]
            kind = "bwrap"
            bin = "bwrap"
            args = []
            stages = []

            [providers.main]
            api = "responses"
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
        println!("{:?}", pool.assistants.keys());
        assert_eq!(pool.switch_assistant("fast", true).unwrap(), "alt");
    }
}

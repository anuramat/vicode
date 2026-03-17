use anyhow::Context;
use anyhow::Result;
use async_openai::types::responses::ReasoningEffort;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use smart_default::SmartDefault;
use xdg::BaseDirectories;

use crate::bwrap::BwrapConfig;

const CONFIG_FILENAME: &str = "config.toml";
const AGENTS_FILENAME: &str = "AGENTS.md"; // in config dir
const XDG_DIRNAME: &str = "vicode";

lazy_static::lazy_static! {
    pub static ref CONFIG: Config = Config::new().unwrap();
    pub static ref DIRS: BaseDirectories = BaseDirectories::with_prefix(XDG_DIRNAME);
    pub static ref INSTRUCTIONS: String = {
        let filepath = DIRS.place_config_file(AGENTS_FILENAME).unwrap();
        std::fs::read_to_string(filepath).unwrap()
    };
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssistantModelConfig {
    pub model: String,
    pub effort: Option<ReasoningEffort>,
}

// TODO make sure these are required
#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiKind {
    #[default]
    Responses,
    ChatCompletions,
}

fn context_files() -> Vec<String> {
    vec!["AGENTS.md".to_string(), "CLAUDE.md".to_string()]
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct ApiCompatConfig {
    pub instructions_as_message: bool,
    pub reasoning_as_output: Option<String>,
    pub developer_as_user: bool,
}

#[derive(Deserialize, Debug, Clone, SmartDefault)]
#[serde(default)]
pub struct ApiConfig {
    pub kind: ApiKind,
    /// base URL for the api; expands env vars
    #[default = "localhost"]
    pub base_url: String,
    /// bash command that outputs the key to stdout
    pub key_command: Option<String>,

    // TODO move to compat?
    /// name of the field with reasoning content (chat completions only)
    pub reasoning_key: Option<String>,

    /// compatibility hacks
    #[serde(flatten)]
    pub compat: ApiCompatConfig,

    /// max number of concurrent requests
    #[default = 1]
    pub concurrency: usize,
    // TODO float
    /// max requests per second
    #[default = 1]
    pub rps: u32,
    /// max retries
    #[default = 5]
    pub retries: usize,
    /// initial retry delay, multiplied by 2 after each attempt
    #[default = 1000]
    pub backoff_ms: u64,
}

impl ApiConfig {
    pub fn base_url(&self) -> Result<String> {
        Ok(shellexpand::full(&self.base_url)?.into_owned())
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct AssistantConfig {
    pub api: String,
    #[serde(flatten)]
    pub model: AssistantModelConfig,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(untagged)]
pub enum SubagentAssistantConfig {
    #[default]
    Inherit,
    Assistants(Vec<String>),
}

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub bash: BashConfig,

    /// AGENTS.md-type files to read from the project root; if multiple are defined, contents are
    /// concatenated
    #[serde(default)]
    pub context_files: Vec<String>,

    pub apis: IndexMap<String, ApiConfig>,
    pub assistants: IndexMap<String, AssistantConfig>,
    pub primary_assistant: Vec<String>,
    #[serde(default)]
    pub subagent_assistant: SubagentAssistantConfig,
}

// TODO recursively drop default?
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(default)]
pub struct BashConfig {
    pub bwrap: BwrapConfig,
    pub cmd: Vec<String>,
}

impl Config {
    fn new() -> Result<Self> {
        let filepath = DIRS.place_config_file(CONFIG_FILENAME)?;
        let s = std::fs::read_to_string(&filepath)
            .with_context(|| format!("failed to read config file at {}", filepath.display()))?;
        Self::parse(&s)
    }

    pub fn parse(s: &str) -> Result<Self> {
        let config: Self = toml::from_str(s)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        for (id, assistant) in &self.assistants {
            anyhow::ensure!(
                self.apis.contains_key(&assistant.api),
                "assistant '{id:?}' references unknown api '{:?}'",
                assistant.api
            );
        }

        self.validate_assistant(&self.primary_assistant)?;
        match &self.subagent_assistant {
            SubagentAssistantConfig::Inherit => {}
            SubagentAssistantConfig::Assistants(ids) => {
                self.validate_assistant(ids)?;
            }
        }
        Ok(())
    }

    fn validate_assistant(
        &self,
        assistant: &Vec<String>,
    ) -> Result<()> {
        anyhow::ensure!(!assistant.is_empty(), "assistant must not be empty");
        for id in assistant {
            anyhow::ensure!(
                self.assistants.contains_key(id),
                "unknown assistant '{id:?}'"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multi_api_config() {
        let config = Config::parse(
            r#"
            primary_assistant = ["fast", "deep"]

            [apis.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rps = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            api = "main"
            model = "gpt-fast"

            [assistants.deep]
            api = "main"
            model = "gpt-deep"
            effort = "low"

            [bash]
            "#,
        )
        .unwrap();
        assert_eq!(config.apis.len(), 1);
        assert_eq!(config.assistants["deep"].api, "main");
    }

    #[test]
    fn rejects_unknown_assistant_reference() {
        let err = Config::parse(
            r#"
            primary_assistant = ["missing"]

            [apis.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rps = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            api = "main"
            model = "gpt-fast"

            [bash]
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown assistant"));
    }
}

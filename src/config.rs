use anyhow::Context;
use anyhow::Result;
use async_openai::types::responses::ReasoningEffort;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use smart_default::SmartDefault;
use xdg::BaseDirectories;

use crate::bwrap::BwrapConfig;
use crate::tui::command::Keymap;

const DEFAULT_INSTRUCTIONS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/default/AGENTS.md"));
const DEFAULT_CONFIG: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/default/config.toml"));
const CONFIG_FILENAME: &str = "config.toml";
const INSTRUCTIONS_FILENAME: &str = "AGENTS.md"; // in config dir
const XDG_DIRNAME: &str = "vicode";

// TODO drop lazy_static, centralize config reading and pass values explicitly
// TODO let user override instructions filepath

lazy_static::lazy_static! {
    pub static ref CONFIG: Config = Config::new().unwrap();
    pub static ref DIRS: BaseDirectories = BaseDirectories::with_prefix(XDG_DIRNAME);
    pub static ref INSTRUCTIONS: String = {
        let filepath = DIRS.place_config_file(INSTRUCTIONS_FILENAME).unwrap();
        if !filepath.exists() {
            std::fs::write(&filepath, DEFAULT_INSTRUCTIONS).unwrap();
        }
        std::fs::read_to_string(filepath).unwrap()
    };
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model: String,
    pub effort: Option<ReasoningEffort>,
    /// max context window
    pub window: Option<usize>,
}

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum ApiType {
    #[default]
    Responses,
    ChatCompletions,
}

fn merge<T>(
    default: T,
    user: T,
) -> Result<Value>
where
    T: Serialize,
{
    let default = serde_json::to_value(default)?;
    let user = serde_json::to_value(user)?;
    match (default, user) {
        (Value::Object(mut default), Value::Object(user)) => {
            default.extend(user);
            Ok(Value::Object(default))
        }
        _ => anyhow::bail!("expected both values to be objects"),
    }
}

#[derive(Deserialize, Debug, Clone, SmartDefault)]
#[serde(default)]
pub struct ApiCompatConfig {
    /// name of the field with reasoning content (chat completions only)
    #[default = "reasoning_content"]
    pub reasoning_content_field: String,
    pub instructions_as_message: bool,
    pub reasoning_as_output: Option<String>,
    pub developer_as_user: bool,
}

#[derive(Deserialize, Debug, Clone, SmartDefault)]
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

impl ProviderConfig {
    pub fn base_url(&self) -> Result<String> {
        Ok(shellexpand::full(&self.base_url)?.into_owned())
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct AssistantConfig {
    pub provider: String,
    #[serde(flatten)]
    pub model: ModelConfig,
}

// TODO don't panic on unknown config fields, just warn

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    /// Paths (relative to project root) to expose in the agent workdir through a special lowerdir shared by all agents.
    /// All directories are assumed to be gitignored. Usecase: compilation cache, .env files etc.
    /// - directories are bind-mounted
    /// - files are hardlinked
    pub shared: Vec<String>,
    pub bash: BashConfig,

    /// AGENTS.md-type files to read from the project root; if multiple are defined, contents are
    /// concatenated
    pub context_files: Vec<String>,

    pub providers: IndexMap<String, ProviderConfig>,
    pub assistants: IndexMap<String, AssistantConfig>,
    // TODO maybe collapse into a struct or something?
    pub primary_assistant: Vec<String>,
    pub subagent_assistant: Vec<String>,

    /// if false, keymaps are merged with defaults
    pub clear_keymap: bool,
    pub keymap: Keymap,
}

// TODO flatten into Config
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct BashConfig {
    pub bwrap: BwrapConfig,
    pub cmd: Vec<String>,
}

impl Config {
    fn new() -> Result<Self> {
        let filepath = DIRS.place_config_file(CONFIG_FILENAME)?;
        if !filepath.exists() {
            // TODO use tokio fs when we stop using lazy_static
            std::fs::write(&filepath, DEFAULT_CONFIG).with_context(|| {
                format!("failed to write default config to {}", filepath.display())
            })?;
        }
        let s = std::fs::read_to_string(&filepath)
            .with_context(|| format!("failed to read config file at {}", filepath.display()))?;
        Self::parse(&s)
    }

    fn default_config() -> Result<Self> {
        let config: Self = toml::from_str(DEFAULT_CONFIG)?;
        config.validate()?;
        Ok(config)
    }

    pub fn parse(s: &str) -> Result<Self> {
        let user: toml::Table = toml::from_str(s)?;
        let default: toml::Table = toml::from_str(DEFAULT_CONFIG)?;
        let merged = merge(default, user)?;
        let mut config: Self = serde_json::from_value(merged)?;
        config.validate()?;

        if !config.clear_keymap {
            // merge keymaps
            let mut keymap = Self::default_config()?.keymap;
            keymap.cmdline.extend(config.keymap.cmdline.clone());
            keymap.normal.extend(config.keymap.normal.clone());
            keymap.insert.extend(config.keymap.insert.clone());
            config.keymap = keymap;
        }

        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        for (id, assistant) in &self.assistants {
            anyhow::ensure!(
                self.providers.contains_key(&assistant.provider),
                "assistant '{id:?}' references unknown provider '{:?}'",
                assistant.provider
            );
        }

        self.validate_assistant(&self.primary_assistant)?;
        if !self.subagent_assistant.is_empty() {
            self.validate_assistant(&self.subagent_assistant)?;
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
    fn parses_default_config() {
        // TODO do properly, ie without unwraps
        let config: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn parses_multi_provider_config() {
        let config = Config::parse(
            r#"
            primary_assistant = ["fast", "deep"]

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

            [bash]
            cmd = ["bash", "-c"]

            [bash.bwrap]
            bin = "bwrap"
            args = []
            stages = []
            "#,
        )
        .unwrap();
        assert!(config.shared.is_empty());
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.assistants["deep"].provider, "main");
    }

    #[test]
    fn rejects_unknown_assistant_reference() {
        let err = Config::parse(
            r#"
            primary_assistant = ["missing"]

            [providers.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rpm = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            provider = "main"
            model = "gpt-fast"

            [bash]
            cmd = ["bash", "-c"]

            [bash.bwrap]
            bin = "bwrap"
            args = []
            stages = []
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn parses_keymap() {
        let config = Config::parse(
            r#"
            primary_assistant = ["fast"]

            [keymap.cmdline]

            [keymap.normal]
            "q" = "quit"
            "1" = "set_multiplier 1"

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

            [bash]
            cmd = ["bash", "-c"]

            [bash.bwrap]
            bin = "bwrap"
            args = []
            stages = []
            "#,
        )
        .unwrap();
        let q = "q".parse::<crate::tui::command::KeyChord>().unwrap();
        let one = "1".parse::<crate::tui::command::KeyChord>().unwrap();
        assert_eq!(
            config.keymap.normal.get(&q).unwrap(),
            &"quit".parse::<crate::tui::command::Command>().unwrap()
        );
        assert_eq!(
            config.keymap.normal.get(&one).unwrap(),
            &"set_multiplier 1"
                .parse::<crate::tui::command::Command>()
                .unwrap()
        );
    }

    #[test]
    fn parses_shift_modifier_in_keymap() {
        let config = Config::parse(
            r#"
            primary_assistant = ["fast"]

            [keymap.cmdline]

            [keymap.normal]
            "S-j" = "tab_next"

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

            [bash]
            cmd = ["bash", "-c"]

            [bash.bwrap]
            bin = "bwrap"
            args = []
            stages = []
            "#,
        )
        .unwrap();
        let shift_j = "S-j".parse::<crate::tui::command::KeyChord>().unwrap();
        assert_eq!(
            config.keymap.normal.get(&shift_j).unwrap(),
            &"tab_next".parse::<crate::tui::command::Command>().unwrap()
        );
    }

    #[test]
    fn rejects_uppercase_keys_in_keymap() {
        let err = Config::parse(
            r#"
            primary_assistant = ["fast"]

            [keymap.cmdline]

            [keymap.normal]
            "Enter" = "input_submit"

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

            [bash]
            cmd = ["bash", "-c"]

            [bash.bwrap]
            bin = "bwrap"
            args = []
            stages = []
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("valid key chord"));
    }

    #[test]
    fn parses_insert_keymap_scope() {
        let config = Config::parse(
            r#"
            primary_assistant = ["fast"]

            [keymap.cmdline]

            [keymap.normal]

            [keymap.insert]
            "enter" = "input_submit"
            "esc" = "input_exit"

            [providers.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rpm = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            provider = "main"
            model = "gpt-fast"

            [bash]
            cmd = ["bash", "-c"]

            [bash.bwrap]
            bin = "bwrap"
            args = []
            stages = []
            "#,
        )
        .unwrap();
        assert_eq!(config.keymap.insert.len(), 2);
    }
}

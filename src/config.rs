use std::collections::HashMap;

use anyhow::Context;
use anyhow::Result;
use schemars::JsonSchema;
use schemars::schema_for;
use serde::Deserialize;
use serde::Serialize;
use smart_default::SmartDefault;
use xdg::BaseDirectories;

pub use crate::llm::provider::ApiCompatConfig;
pub use crate::llm::provider::ApiType;
pub use crate::llm::provider::ProviderConfig;
pub use crate::llm::provider::assistant::AssistantConfig;
pub use crate::llm::provider::assistant::ModelConfig;
pub use crate::sandbox::SandboxConfig;
use crate::tui::command::Keymap;
use crate::tui::widgets::container::element::RenderContext;

const DEFAULT_INSTRUCTIONS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/default/AGENTS.md"));
const DEFAULT_CONFIG: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/default/config.toml"));
const CONFIG_FILENAME: &str = "config.toml";
const INSTRUCTIONS_FILENAME: &str = "AGENTS.md"; // in config dir
const XDG_DIRNAME: &str = "vicode";
const SCHEMA_FILENAME: &str = "schema.json";

// TODO try to drop LazyLock, centralize config reading and pass values explicitly

pub static CONFIG: std::sync::LazyLock<Config> =
    std::sync::LazyLock::new(|| Config::new().unwrap());
pub static DIRS: std::sync::LazyLock<BaseDirectories> =
    std::sync::LazyLock::new(|| BaseDirectories::with_prefix(XDG_DIRNAME));
pub static INSTRUCTIONS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let filepath = DIRS.place_config_file(INSTRUCTIONS_FILENAME).unwrap();
    if !filepath.exists() {
        std::fs::write(&filepath, DEFAULT_INSTRUCTIONS).unwrap();
    }
    std::fs::read_to_string(filepath).unwrap()
});

#[derive(Deserialize, Debug, Clone, SmartDefault, Serialize, JsonSchema)]
pub struct CompactConfig {
    /// context window percentage, at which we compact the context, threshold < 100
    #[default = 80]
    pub threshold: usize,
    /// context window percentage to compact to
    /// we compact the first N messages, where N is the smallest number s.t. `old_total - dropped < target_percentage * context_window`
    /// note that compacted messages take tokens, so this doesn't guarantee that we will be below target in the end
    #[default = 20]
    pub target: usize,
}

pub fn expand_vec<I>(values: I) -> Vec<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    values
        .into_iter()
        .flat_map(|value| shellexpand::full(value.as_ref()).map(String::from).ok())
        .collect()
}

pub fn vec<I>(values: I) -> Vec<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    values.into_iter().map(|x| x.as_ref().to_string()).collect()
}

#[derive(Deserialize, Debug, SmartDefault, Serialize, JsonSchema)]
#[serde(default)]
pub struct Config {
    /// disable fuse-overlayfs/bindfs overlays and just copy stuff around; mac compatibility hack
    pub disable_overlay: bool,
    pub sandbox: SandboxConfig,
    #[default(vec(["bash", "-c"]))]
    pub shell_cmd: Vec<String>,
    /// Paths (relative to project root) to expose in the agent workdir through a special lowerdir shared by all agents.
    /// Usecase: compilation cache, .env files etc.
    /// - ignored on mac
    /// - paths must be gitignored
    /// - paths must not be modified in the repo while the app is running
    /// - directories are bind-mounted
    /// - files are hardlinked
    pub shared: Vec<String>,

    /// AGENTS.md-type files to read from the project root; if multiple are defined, contents are
    /// concatenated
    #[default(vec(["AGENTS.md"]))]
    pub context_files: Vec<String>,

    /// rendering options; can be toggled at runtime
    pub render: RenderContext,

    pub providers: HashMap<String, ProviderConfig>,
    pub assistants: HashMap<String, AssistantConfig>,

    // TODO maybe collapse into a struct or something?
    /// list of assistants for new tabs (round robin)
    pub primary_assistant: Vec<String>,
    /// if empty, inherits from its parent
    pub subagent_assistant: Vec<String>,

    pub keymap: Keymap,

    pub compact: CompactConfig,
}

impl std::fmt::Display for Config {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        write!(
            f,
            "{}",
            toml::to_string_pretty(self).map_err(|_| std::fmt::Error)?
        )
    }
}

impl Config {
    // TODO check if schema is up to date, rewrite if not
    fn put_schema() -> Result<()> {
        let filepath = DIRS.place_config_file(SCHEMA_FILENAME)?;
        if !filepath.exists() {
            let schema = schema_for!(Config);
            std::fs::write(&filepath, serde_json::to_string_pretty(&schema)?).with_context(
                || format!("failed to write config schema to {}", filepath.display()),
            )?;
        }
        Ok(())
    }

    fn new() -> Result<Self> {
        Self::put_schema()?;
        let filepath = DIRS.place_config_file(CONFIG_FILENAME)?;
        if !filepath.exists() {
            // TODO use tokio fs when we stop using lazy_static
            std::fs::write(&filepath, DEFAULT_CONFIG).with_context(|| {
                format!("failed to write default config to {}", filepath.display())
            })?;
        }
        let s = std::fs::read_to_string(&filepath)
            .with_context(|| format!("failed to read config file at {}", filepath.display()))?;
        Self::parse_with_defaults(&s)
    }

    pub fn parse_with_defaults(s: &str) -> Result<Self> {
        let mut config: Self = toml::from_str(s)?;
        config.keymap = config.keymap.maybe_with_defaults();
        config.sandbox.maybe_with_defaults();
        config.validate()?;
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
        let config: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn parses_multi_provider_config() {
        let config = Config::parse_with_defaults(
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
        assert!(config.shared.is_empty());
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.assistants["deep"].provider, "main");
    }

    #[test]
    fn rejects_unknown_assistant_reference() {
        let err = Config::parse_with_defaults(
            r#"
            primary_assistant = ["missing"]
            shell_cmd = ["bash", "-c"]

            [sandbox]
            kind = "bwrap"
            bin = "bwrap"
            args = []
            stages = []

            [providers.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rpm = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            provider = "main"
            model = "gpt-fast"

            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn parses_keymap() {
        let config = Config::parse_with_defaults(
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
        let colon = ":".parse::<crate::tui::command::KeyChord>().unwrap();
        assert_eq!(
            config.keymap.normal.get(&colon).unwrap(),
            &"cmdline_enter"
                .parse::<crate::tui::command::Command>()
                .unwrap()
        );
    }

    #[test]
    fn parses_shift_modifier_in_keymap() {
        let config = Config::parse_with_defaults(
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
    fn parses_insert_keymap_scope() {
        let config = Config::parse_with_defaults(
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

            "#,
        )
        .unwrap();
        assert_eq!(config.keymap.insert.len(), 2);
    }

    #[test]
    fn requires_explicit_assistant_setup() {
        let err = Config::parse_with_defaults(
            r#"
            shared = [".cache"]
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("assistant must not be empty"));
    }

    #[test]
    fn clear_keymap_replaces_defaults() {
        let config = Config::parse_with_defaults(
            r#"
            primary_assistant = ["fast"]

            [sandbox]
            kind = "bwrap"
            bin = "bwrap"
            args = []
            stages = []

            [keymap]
            clear_defaults = true
            [keymap.normal]
            "q" = "quit"

            [providers.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rpm = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            provider = "main"
            model = "gpt-fast"
            "#,
        )
        .unwrap();
        let q = "q".parse::<crate::tui::command::KeyChord>().unwrap();
        let colon = ":".parse::<crate::tui::command::KeyChord>().unwrap();
        assert!(config.keymap.normal.contains_key(&q));
        assert!(!config.keymap.normal.contains_key(&colon));
    }
}

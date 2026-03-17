use anyhow::Context;
use anyhow::Result;
use async_openai::types::responses::ReasoningEffort;
use serde::Deserialize;
use serde::Serialize;
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

#[derive(Deserialize, Debug, Clone)]
pub struct ApiConfig {
    #[serde(default)]
    pub kind: ApiKind,
    pub base_url: String,
    pub model_name: String,
    pub key_command: Option<String>,

    pub effort: Option<ReasoningEffort>,

    /// name of the field with reasoning content (chat completions only)
    pub reasoning_key: Option<String>,

    /// compatibility hacks
    #[serde(flatten)]
    pub compat: ApiCompatConfig,

    /// max number of concurrent requests
    pub concurrency: usize,
    /// max requests per second
    pub rps: u32,
    /// max retries
    pub retries: u32,
    /// initial retry delay, multiplied by 2 after each attempt
    pub backoff_ms: u64,
}

impl ApiConfig {
    pub fn base_url(&self) -> Result<String> {
        Ok(shellexpand::full(&self.base_url)?.into_owned())
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub bash: BashConfig,
    /// fallbacks for AGENTS.md
    #[serde(default = "context_files")]
    pub context_files: Vec<String>,

    pub api: ApiConfig,
}

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
        Ok(toml::from_str(&s)?)
    }
}

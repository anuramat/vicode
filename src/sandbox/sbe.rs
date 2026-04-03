use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use smart_default::SmartDefault;

use super::SandboxRunner;

#[derive(Debug, Clone, SmartDefault, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SbeConfig {
    #[default("sandbox-exec")]
    pub bin: String,
    pub profile: String,
}

#[derive(Debug, Clone)]
pub struct SbeRunner {
    pub bin: String,
    pub args: Vec<String>,
    pub cwd: std::path::PathBuf,
}

impl crate::sandbox::Sandbox for SbeConfig {
    fn runner(
        &self,
        cwd: std::path::PathBuf,
        _gitdir: std::path::PathBuf,
    ) -> SandboxRunner {
        SandboxRunner {
            bin: self.bin.clone(),
            args: vec!["-p".to_string(), self.profile.clone()],
            cwd,
        }
    }
}

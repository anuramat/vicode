pub mod bwrap;
pub mod sbe;

use std::path::PathBuf;
use std::process::Output;

use anyhow::Result;
use dyn_clone::DynClone;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::config::CONFIG;
use crate::sandbox::bwrap::BwrapConfig;
use crate::sandbox::sbe::SbeConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
#[serde(default)]
pub struct SandboxConfig {
    bwrap: BwrapConfig,
    sbe: SbeConfig,
    /// path to a custom sandbox script
    custom: Option<String>,
}

pub trait Sandbox: std::fmt::Debug + DynClone + Send + Sync {
    fn runner(
        &self,
        cwd: PathBuf,
        gitdir: PathBuf,
    ) -> SandboxRunner;
}

#[derive(Debug, Clone)]
pub struct SandboxRunner {
    pub bin: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl Sandbox for SandboxConfig {
    fn runner(
        &self,
        cwd: PathBuf,
        gitdir: PathBuf,
    ) -> SandboxRunner {
        if let Some(custom) = &self.custom {
            SandboxRunner {
                bin: custom.clone(),
                args: vec![],
                cwd,
            }
        } else {
            // default to bwrap
            // TODO: choose depending on the platform
            self.bwrap.runner(cwd, gitdir)
        }
    }
}

impl SandboxRunner {
    pub async fn exec(
        &self,
        script: String,
    ) -> Result<Output> {
        let mut bash_cmd = CONFIG.shell_cmd.clone();
        bash_cmd.push(script);

        let mut bwrap_bin = tokio::process::Command::new(&self.bin);
        Ok(bwrap_bin
            .current_dir(&self.cwd)
            .args(&self.args)
            .args(bash_cmd.into_iter())
            .output()
            .await?)
    }
}

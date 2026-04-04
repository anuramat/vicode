pub mod bwrap;
pub mod sbe;

use std::path::PathBuf;
use std::process::Output;

use anyhow::Result;
use dyn_clone::DynClone;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::sandbox::bwrap::BwrapConfig;
use crate::sandbox::sbe::SbeConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
#[serde(default)]
pub struct SandboxConfig {
    clear_defaults: bool,
    bwrap: BwrapConfig,
    sbe: SbeConfig,
    /// path to a custom sandbox script
    custom: Option<String>,
}

impl SandboxConfig {
    pub fn maybe_with_defaults(&mut self) {
        if !self.clear_defaults {
            self.bwrap.with_defaults();
        }
    }
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
        mut shell_cmd: Vec<String>,
        script: String,
    ) -> Result<Output> {
        shell_cmd.push(script);
        let mut cmd = tokio::process::Command::new(&self.bin);
        Ok(cmd
            .current_dir(&self.cwd)
            .args(&self.args)
            .args(shell_cmd.into_iter())
            .output()
            .await?)
    }
}

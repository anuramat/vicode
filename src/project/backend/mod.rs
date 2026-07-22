pub mod cow;
pub mod overlay;

use std::path::PathBuf;

use ambassador::Delegate;
use ambassador::delegatable_trait;
use anyhow::Result;

use crate::agent::AgentId;
use crate::config::Config;
use crate::project::Paths;
use crate::sandbox::SandboxConfig;
use crate::sandbox::SandboxRunner;

#[derive(Debug, Clone, Delegate)]
#[delegate(BackendOps)]
pub enum Backend {
    Overlay(Overlay),
    Cow(Cow),
}

#[derive(Debug, Clone)]
pub struct Overlay {
    pub sandbox: SandboxConfig,
    pub shared_paths: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct Cow {
    pub sandbox: SandboxConfig,
}

impl Backend {
    pub fn from_config(config: &Config) -> Self {
        let sandbox = config.sandbox.clone();
        if cfg!(target_os = "macos") {
            Self::Cow(Cow { sandbox })
        } else if cfg!(target_os = "linux") {
            Self::Overlay(Overlay {
                sandbox,
                shared_paths: config.shared.clone(),
            })
        } else {
            unreachable!("compile_error! in main.rs should have fired for this target_os")
        }
    }

    /// paths excluded from diffs etc
    pub fn excluded_workdir_paths(&self) -> &[String] {
        match self {
            Self::Overlay(overlay) => &overlay.shared_paths,
            Self::Cow(_) => &[],
        }
    }
}

#[async_trait::async_trait]
#[delegatable_trait]
pub trait BackendOps {
    fn sandbox_runner(
        &self,
        cwd: PathBuf,
        gitdir: PathBuf,
    ) -> SandboxRunner;
    async fn init(
        &self,
        paths: &Paths,
    ) -> Result<()>;
    /// initialize a clean workdir at a given commit
    async fn new_agent_workdir(
        &self,
        paths: &Paths,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()>;
    async fn mount_agent(
        &self,
        paths: &Paths,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()>;
    async fn unmount_agent(
        &self,
        paths: &Paths,
        aid: &AgentId,
    ) -> Result<()>;
    async fn unmount_all(
        &self,
        paths: &Paths,
    ) -> Result<()>;
    async fn duplicate_agent_workdir(
        &self,
        paths: &Paths,
        src_aid: &AgentId,
        dst_aid: &AgentId,
        commit: &str,
    ) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Overlay {
        pub fn test() -> Self {
            Self {
                sandbox: Config::test().sandbox,
                shared_paths: Vec::new(),
            }
        }
    }
}

pub mod copy;
pub mod overlay;

use std::path::PathBuf;

use ambassador::Delegate;
use ambassador::delegatable_trait;
use anyhow::Result;

use crate::agent::AgentId;
use crate::config::Config;
use crate::project::Layout;
use crate::sandbox::SandboxConfig;
use crate::sandbox::SandboxRunner;

// TODO drop the enum and use some macro?
#[derive(Debug, Clone, Delegate)]
#[delegate(WorkspaceBackend)]
pub enum BackendKind {
    Overlay(Overlay),
    Copy(Copy),
}

#[derive(Debug, Clone)]
pub struct Overlay {
    pub sandbox: SandboxConfig,
}
#[derive(Debug, Clone)]
pub struct Copy {
    pub sandbox: SandboxConfig,
}

impl BackendKind {
    pub fn from_config(config: &Config) -> Self {
        let sandbox = config.sandbox.clone();
        if config.disable_overlay {
            Self::Copy(Copy { sandbox })
        } else {
            Self::Overlay(Overlay { sandbox })
        }
    }
}

#[async_trait::async_trait]
#[delegatable_trait]
pub trait WorkspaceBackend {
    fn agent_diff_root(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf;
    fn sandbox_runner(
        &self,
        cwd: PathBuf,
        gitdir: PathBuf,
    ) -> SandboxRunner;
    async fn init(
        &self,
        layout: &Layout,
        config: &Config,
    ) -> Result<()>;
    async fn new_agent_workdir(
        &self,
        layout: &Layout,
        commit: &str,
        aid: &AgentId,
        git: bool,
    ) -> Result<()>;
    async fn mount_agent(
        &self,
        layout: &Layout,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()>;
    async fn unmount_agent(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> Result<()>;
    async fn unmount_all(
        &self,
        layout: &Layout,
    ) -> Result<()>;
    async fn duplicate_agent_workdir(
        &self,
        layout: &Layout,
        src_aid: &AgentId,
        dst_aid: &AgentId,
        commit: &str,
        git: bool,
    ) -> Result<()>;
}

pub mod copy;
pub mod overlay;

use std::path::PathBuf;

use ambassador::Delegate;
use ambassador::delegatable_trait;
use anyhow::Result;

use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::config::Config;
use crate::project::Layout;

// TODO drop the enum and use some macro?
#[derive(Debug, Clone, Delegate)]
#[delegate(Backend)]
pub enum BackendKind {
    Overlay(Overlay),
    Copy(Copy),
}

#[derive(Debug, Clone)]
pub struct Overlay;
#[derive(Debug, Clone)]
pub struct Copy;

#[async_trait::async_trait]
#[delegatable_trait]
pub trait Backend {
    fn agent_changes_dir(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf;
    async fn init(
        &self,
        layout: &Layout,
        config: &Config,
    ) -> Result<()>;
    async fn new_agent(
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
    async fn duplicate_agent(
        &self,
        layout: &Layout,
        src_id: &AgentId,
        aid: &AgentId,
        state: &AgentState,
        git: bool,
    ) -> Result<()>;
}

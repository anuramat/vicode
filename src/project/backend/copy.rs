use std::path::PathBuf;

use anyhow::Result;
use tokio::fs::create_dir_all;

use crate::agent::id::AgentId;
use crate::git;
use crate::git::checkout;
use crate::git::worktree;
use crate::project::Layout;
use crate::project::backend::WorkspaceBackend;
use crate::project::layout::LayoutTrait;
use crate::sandbox::Sandbox;
use crate::sandbox::SandboxRunner;

#[async_trait::async_trait]
impl WorkspaceBackend for super::Copy {
    fn agent_diff_root(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf {
        layout.agent_workdir(aid)
    }

    fn sandbox_runner(
        &self,
        cwd: PathBuf,
        gitdir: PathBuf,
    ) -> SandboxRunner {
        self.sandbox.runner(cwd, gitdir)
    }

    async fn init(
        &self,
        _layout: &Layout,
        _config: &crate::config::Config,
    ) -> Result<()> {
        Ok(())
    }

    async fn new_agent_workdir(
        &self,
        layout: &Layout,
        commit: &str,
        aid: &AgentId,
        git: bool,
    ) -> Result<()> {
        if git {
            worktree(layout, aid, commit, true).await?;
        } else {
            let path = layout.agent_workdir(aid);
            checkout(layout, commit, path).await?;
        }
        Ok(())
    }

    async fn mount_agent(
        &self,
        _layout: &Layout,
        _commit: &str,
        _aid: &AgentId,
    ) -> Result<()> {
        Ok(())
    }

    async fn unmount_agent(
        &self,
        _layout: &Layout,
        _aid: &AgentId,
    ) -> Result<()> {
        Ok(())
    }

    async fn unmount_all(
        &self,
        _layout: &Layout,
    ) -> Result<()> {
        Ok(())
    }

    async fn duplicate_agent_workdir(
        &self,
        layout: &Layout,
        src_aid: &AgentId,
        dst_aid: &AgentId,
        commit: &str,
        git: bool,
    ) -> Result<()> {
        let from = layout.agent_workdir(src_aid);
        let to = layout.agent_workdir(dst_aid);
        if git {
            git::worktree(layout, dst_aid, commit, false).await?;
        } else {
            create_dir_all(to.clone()).await?;
        }
        crate::git::copy_without_dot_git(&from, to).await?;
        Ok(())
    }
}

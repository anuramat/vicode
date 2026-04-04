use std::path::PathBuf;

use anyhow::Result;
use tokio::fs::create_dir_all;

use crate::agent::id::AgentId;
use crate::agent::*;
use crate::git;
use crate::git::checkout;
use crate::git::worktree;
use crate::project::Layout;
use crate::project::backend::Backend;
use crate::project::layout::LayoutTrait;

#[async_trait::async_trait]
impl Backend for super::Copy {
    fn agent_changes_dir(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf {
        layout.agent_workdir(aid)
    }

    async fn init(
        &self,
        _layout: &Layout,
        _config: &crate::config::Config,
    ) -> Result<()> {
        Ok(())
    }

    async fn new_agent(
        &self,
        layout: &Layout,
        commit: &str,
        aid: &AgentId,
        git: bool,
    ) -> Result<()> {
        if !git {
            let path = layout.agent_workdir(aid);
            checkout(layout, commit, path).await?;
        } else {
            worktree(layout, aid, commit, true).await?;
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

    async fn duplicate_agent(
        &self,
        layout: &Layout,
        src_id: &AgentId,
        aid: &AgentId,
        state: &AgentState,
        git: bool,
    ) -> Result<()> {
        let from = layout.agent_workdir(src_id);
        let to = layout.agent_workdir(aid);
        if !git {
            create_dir_all(to.clone()).await?;
        } else {
            git::worktree(layout, aid, &state.context.commit, false).await?;
        }
        crate::git::copy_without_dot_git(&from, to).await?;
        Ok(())
    }
}

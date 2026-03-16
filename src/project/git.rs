use anyhow::Result;

use super::Project;
use crate::agent::AgentId;
use crate::git::worktree_no_checkout;

impl Project {
    pub async fn ensure_snapshot(
        &self,
        commit: &str,
    ) -> Result<()> {
        let path = self.snapshot(commit);
        if path.exists() {
            return Ok(());
        }
        let args = [
            "worktree",
            "add",
            "--detach",
            &path.to_string_lossy(),
            commit,
        ];
        self.bash("git", args).await?.status.exit_ok()?;
        Ok(())
    }

    /// prepare the worktree overlayfs directories:
    /// identical to a normal worktree, but reusing snapshot to save storage
    pub async fn add_worktree(
        &self,
        aid: &AgentId,
        commit: &str,
    ) -> Result<()> {
        let name = Self::worktree_name(aid);
        worktree_no_checkout(&self.root, &name, &self.agent_workdir(aid), commit).await?;
        tokio::fs::rename(
            self.agent_workdir(aid).join(".git"),
            self.overlay_upper(aid).join(".git"),
        )
        .await?;
        Ok(())
    }

    pub async fn whiteout_git(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        self.whiteout(aid, ".git").await
    }
}

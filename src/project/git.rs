use std::process::Command;
use std::process::Stdio;

use anyhow::Result;
use anyhow::anyhow;
use tokio::fs::create_dir_all;

use super::Project;
use crate::agent::id::AgentId;
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
        create_dir_all(&path).await?;

        let dest = path.clone();
        let root = self.root.clone();
        let commit = commit.to_string();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut archive = Command::new("git")
                .current_dir(root)
                .args(["archive", &commit])
                .stdout(Stdio::piped())
                .spawn()?;
            let tar = Command::new("tar")
                .arg("-x")
                .arg("-C")
                .arg(&dest)
                .stdin(
                    archive
                        .stdout
                        .take()
                        .ok_or_else(|| anyhow!("missing git archive stdout"))?,
                )
                .status()?;
            archive.wait()?.exit_ok()?;
            tar.exit_ok()?;
            Ok(())
        })
        .await??;
        Ok(())
    }

    // TODO this should be renamed
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
}

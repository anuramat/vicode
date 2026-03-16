use anyhow::Result;
use anyhow::bail;
use git2::Repository;
use tokio::fs::create_dir_all;

use super::Project;
use crate::agent::*;

impl Project {
    pub async fn whiteout(
        &self,
        aid: &AgentId,
        file: &str, // relative to root of agent workdir
    ) -> Result<()> {
        let path = self
            .overlay_upper(aid)
            .join(file)
            .to_string_lossy()
            .to_string();
        let args = ["-m=000", &path, "c", "0", "0"];
        self.bash("mknod", args).await?.status.exit_ok()?;
        Ok(())
    }

    pub async fn unmount(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        let output = self
            .bash(
                "umount",
                [self.agent_workdir(aid).to_string_lossy().to_string()],
            )
            .await?;
        match output.status.code() {
            Some(0) => (),
            Some(1) => return Ok(()),  // already unmounted
            Some(32) => return Ok(()), // busy
            _ => {
                bail!("unexpected umount output: {:?}", output)
            }
        }
        Ok(())
    }

    pub async fn init_overlay(
        &self,
        commit: &str,
        aid: &AgentId,
        git: bool,
    ) -> Result<()> {
        create_dir_all(self.overlay_workdir(aid)).await?;
        create_dir_all(self.overlay_upper(aid)).await?;
        self.ensure_snapshot(commit).await?;

        if !git {
            create_dir_all(self.agent_workdir(aid)).await?;
            self.whiteout_git(aid).await?;
            return Ok(());
        }

        self.add_worktree(aid, commit).await?;

        self.mount(commit, aid).await?;
        let repo = Repository::open(self.agent_workdir(aid))?;
        let oid = git2::Oid::from_str(commit)?;
        let target = repo.find_object(oid, None)?;
        repo.reset(&target, git2::ResetType::Mixed, None)?;

        Ok(())
    }

    async fn mount_status(
        &self,
        aid: &AgentId,
    ) -> Result<MountStatus> {
        let output = self
            .bash(
                "mountpoint",
                [self.agent_workdir(aid).to_string_lossy().to_string()],
            )
            .await?;
        let status = match output.status.code() {
            Some(0) => MountStatus::Mounted,
            Some(1) => MountStatus::Broken,
            Some(32) => MountStatus::Unmounted,
            _ => {
                bail!("unexpected mountpoint output: {:?}", output)
            }
        };
        Ok(status)
    }

    pub async fn mount(
        &self,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()> {
        match self.mount_status(aid).await? {
            MountStatus::Mounted => return Ok(()),
            MountStatus::Broken => self.unmount(aid).await?,
            MountStatus::Unmounted => (),
        }

        let options = format!(
            "lowerdir={},upperdir={},workdir={}",
            self.snapshot(commit).to_string_lossy(),
            self.overlay_upper(aid).to_string_lossy(),
            self.overlay_workdir(aid).to_string_lossy(),
        );
        let args = [
            "-o".to_string(),
            options,
            self.agent_workdir(aid).to_string_lossy().to_string(),
        ];
        self.bash("fuse-overlayfs", args).await?.status.exit_ok()?;
        Ok(())
    }
}

enum MountStatus {
    Mounted,
    Unmounted,
    Broken,
}

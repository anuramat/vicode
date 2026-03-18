use std::path::Path;

use anyhow::Result;
use anyhow::bail;
use git2::Repository;
use proc_mounts::MountIter;
use tokio::fs::create_dir_all;
use tokio::fs::hard_link;

use super::Project;
use crate::agent::*;
use crate::config::CONFIG;
use crate::project::layout::AGENT_WORKDIR_DIRNAME;

// TODO this file is kinda huge. probably should move it to a separate module and/or split

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

    // XXX maybe drop this one
    pub async fn unmount(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        self.unmount_path(&self.agent_workdir(aid)).await
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

    pub async fn init_shared_lowerdir(&self) -> Result<()> {
        self.cleanup_shared().await?;
        create_dir_all(self.shared_root()).await?;
        for path in &CONFIG.shared {
            let path = Path::new(path);
            let src = self.root.join(path);
            if !src.exists() {
                continue;
            }

            let dst = self.shared(path);
            if src.is_dir() {
                self.add_shared_dir(&src, &dst).await?;
            } else {
                self.add_shared_file(&src, &dst).await?;
            }
        }
        Ok(())
    }

    pub async fn cleanup_shared(&self) -> Result<()> {
        for mount in MountIter::new()? {
            if let Ok(mount) = mount
                && mount.fstype == "fuse"
                && mount.dest.starts_with(self.shared_root())
            {
                self.unmount_path(Path::new(&mount.dest)).await?;
            }
        }
        if self.shared_root().exists() {
            tokio::fs::remove_dir_all(self.shared_root()).await?;
        }
        Ok(())
    }

    pub async fn unmount_agent_overlays(&self) -> Result<()> {
        for mount in MountIter::new()? {
            if let Ok(mount) = mount
                && mount.fstype == "fuse.fuse-overlayfs"
                && mount.dest.starts_with(self.agents())
                && mount.dest.ends_with(AGENT_WORKDIR_DIRNAME)
            {
                self.unmount_path(Path::new(&mount.dest)).await?;
            }
        }
        Ok(())
    }

    pub async fn cleanup(&self) -> Result<()> {
        self.cleanup_shared().await?;
        self.unmount_agent_overlays().await?;
        Ok(())
    }

    async fn mount_status(
        &self,
        path: &Path,
    ) -> Result<MountStatus> {
        let output = self
            .bash("mountpoint", [path.to_string_lossy().to_string()])
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
        match self.mount_status(&self.agent_workdir(aid)).await? {
            MountStatus::Mounted | MountStatus::Broken => self.unmount(aid).await?,
            MountStatus::Unmounted => (),
        }

        let options = overlay_options(
            &self.snapshot(commit),
            &self.shared_root(),
            &self.overlay_upper(aid),
            &self.overlay_workdir(aid),
        );
        let args = [
            "-o".to_string(),
            options,
            self.agent_workdir(aid).to_string_lossy().to_string(),
        ];
        self.bash("fuse-overlayfs", args).await?.status.exit_ok()?;
        Ok(())
    }

    async fn add_shared_file(
        &self,
        src: &Path,
        dst: &Path,
    ) -> Result<()> {
        if let Some(parent) = dst.parent() {
            create_dir_all(parent).await?;
        }
        // XXX do we need to create parent dir or maybe hard_link does it automatically?
        hard_link(src, dst).await?;
        Ok(())
    }

    async fn add_shared_dir(
        &self,
        src: &Path,
        dst: &Path,
    ) -> Result<()> {
        create_dir_all(dst).await?;
        let args = [
            src.to_string_lossy().to_string(),
            dst.to_string_lossy().to_string(),
            "--no-allow-other".to_string(),
        ];
        self.bash("bindfs", args).await?.status.exit_ok()?;
        Ok(())
    }

    async fn unmount_path(
        &self,
        path: &Path,
    ) -> Result<()> {
        self.bash("umount", [path.to_string_lossy().to_string()])
            .await?
            .status
            .exit_ok()?;
        Ok(())
    }
}

enum MountStatus {
    Mounted,
    Unmounted,
    Broken,
}

fn overlay_options(
    snapshot: &Path,
    shared: &Path,
    upper: &Path,
    workdir: &Path,
) -> String {
    format!(
        "lowerdir={}:{},upperdir={},workdir={}",
        snapshot.to_string_lossy(),
        shared.to_string_lossy(),
        upper.to_string_lossy(),
        workdir.to_string_lossy(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_mount_orders_snapshot_before_shared() {
        let options = overlay_options(
            Path::new("/snapshot"),
            Path::new("/shared"),
            Path::new("/upper"),
            Path::new("/work"),
        );
        assert!(options.contains("lowerdir=/snapshot:/shared"));
    }
}

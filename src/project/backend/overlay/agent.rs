use std::path::Path;

use anyhow::Result;
use git2::Repository;
use tokio::fs::create_dir_all;

use super::Overlay;
use crate::agent::id::AgentId;
use crate::git::checkout;
use crate::git::worktree;
use crate::project::Layout;
use crate::project::layout::AGENT_WORKDIR_DIRNAME;
use crate::project::layout::LayoutTrait;

impl Overlay {
    /// create agent workdir, maybe with a git worktree; the run loop owns mounting
    pub async fn init_overlay(
        &self,
        layout: &Layout,
        commit: &str,
        aid: &AgentId,
        git: bool,
    ) -> Result<()> {
        create_dir_all(self.overlay_workdir(layout, aid)).await?;
        create_dir_all(self.overlay_upper(layout, aid)).await?;
        self.ensure_snapshot(layout, commit).await?;

        if !git {
            create_dir_all(layout.agent_workdir(aid)).await?;
            return Ok(());
        }

        worktree(layout, aid, commit, false).await?;
        // mixed reset only writes HEAD and the index, so it works before the
        // overlay is mounted, while the worktree is still empty
        {
            let repo = Repository::open(layout.agent_workdir(aid))?;
            let oid = git2::Oid::from_str(commit)?;
            let target = repo.find_object(oid, None)?;
            repo.reset(&target, git2::ResetType::Mixed, None)?;
        }
        tokio::fs::rename(
            layout.agent_workdir(aid).join(".git"),
            self.overlay_upper(layout, aid).join(".git"),
        )
        .await?;

        Ok(())
    }

    pub async fn ensure_snapshot(
        &self,
        layout: &Layout,
        commit: &str,
    ) -> Result<()> {
        let path = self.snapshot(layout, commit);
        if path.exists() {
            return Ok(());
        }
        checkout(layout, commit, path).await?;
        Ok(())
    }

    pub fn overlay_options(
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

    pub async fn unmount_agents(
        &self,
        layout: &Layout,
    ) -> Result<()> {
        for mount in proc_mounts::MountIter::new()? {
            if let Ok(mount) = mount
                && mount.fstype == "fuse.fuse-overlayfs"
                && mount.dest.starts_with(layout.agents())
                && mount.dest.ends_with(AGENT_WORKDIR_DIRNAME)
            {
                self.unmount(layout, Path::new(&mount.dest)).await?;
            }
        }
        Ok(())
    }
}

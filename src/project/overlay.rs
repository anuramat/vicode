use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use git2::Repository;
use proc_mounts::MountIter;
use tokio::fs::create_dir_all;
use tokio::fs::hard_link;

use super::Overlay;
use crate::agent::id::AgentId;
use crate::agent::*;
use crate::config::CONFIG;
use crate::git::worktree_no_checkout;
use crate::project::Backend;
use crate::project::Layout;
use crate::project::layout::AGENT_WORKDIR_DIRNAME;
use crate::project::layout::LayoutTrait;

// TODO this file is kinda huge. probably should move it to a separate module and/or split

const OVERLAY_DIRNAME: &str = ".overlay";
const OVERLAY_UPPER_DIRNAME: &str = "upper";
const OVERLAY_WORKDIR_DIRNAME: &str = "workdir";
const SNAPSHOTS_DIRNAME: &str = "snapshots";
const SHARED_DIRNAME: &str = "shared";

enum MountStatus {
    Mounted,
    Unmounted,
    Broken,
}

#[async_trait::async_trait]
impl Backend for Overlay {
    fn agent_changes_dir(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf {
        self.overlay_upper(layout, aid)
    }

    async fn init(
        &self,
        layout: &Layout,
    ) -> Result<()> {
        std::fs::create_dir_all(self.snapshots(layout))?;
        self.init_shared_lowerdir(layout).await
    }

    async fn new_agent(
        &self,
        layout: &Layout,
        commit: &str,
        aid: &AgentId,
        git: bool,
    ) -> Result<()> {
        self.init_overlay(layout, commit, aid, git).await
    }

    async fn mount_agent(
        &self,
        layout: &Layout,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()> {
        self.mount(layout, commit, aid).await
    }

    async fn unmount_agent(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> Result<()> {
        self.unmount(layout, aid).await
    }

    async fn unmount_all(
        &self,
        layout: &Layout,
    ) -> Result<()> {
        self.cleanup(layout).await
    }

    async fn duplicate_agent(
        &self,
        layout: &Layout,
        src_id: &AgentId,
        aid: &AgentId,
        state: &AgentState,
        git: bool,
    ) -> Result<()> {
        use fs_extra::dir::copy;
        {
            let src = self.overlay_upper(layout, src_id);
            let dst = self.overlay_upper(layout, aid);
            let opts = fs_extra::dir::CopyOptions::new().copy_inside(true);
            copy(src, dst, &opts)?;
            tokio::fs::remove_file(self.overlay_upper(layout, aid).join(".git")).await?;
        }
        self.init_overlay(layout, &state.context.commit, aid, git)
            .await?;
        state.save(aid).await
    }

    // TODO this should be renamed
    /// prepare the worktree overlayfs directories:
    /// identical to a normal worktree, but reusing snapshot to save storage
    async fn add_worktree(
        &self,
        layout: &Layout,
        aid: &AgentId,
        commit: &str,
        name: &str,
    ) -> Result<()> {
        worktree_no_checkout(&layout.root, name, &layout.agent_workdir(aid), commit).await?;
        tokio::fs::rename(
            layout.agent_workdir(aid).join(".git"),
            self.overlay_upper(layout, aid).join(".git"),
        )
        .await?;
        Ok(())
    }
}

impl Overlay {
    pub async fn unmount(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> Result<()> {
        self.unmount_path(layout, &layout.agent_workdir(aid)).await
    }

    /// create and mount agent workdir, maybe with a git worktree
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
            self.mount(layout, commit, aid).await?;
            return Ok(());
        }

        self.add_worktree(layout, aid, commit, &layout.worktree_name(aid))
            .await?;

        self.mount(layout, commit, aid).await?;
        let repo = Repository::open(layout.agent_workdir(aid))?;
        let oid = git2::Oid::from_str(commit)?;
        let target = repo.find_object(oid, None)?;
        repo.reset(&target, git2::ResetType::Mixed, None)?;

        Ok(())
    }

    pub async fn init_shared_lowerdir(
        &self,
        layout: &Layout,
    ) -> Result<()> {
        self.cleanup_shared(layout).await?;
        create_dir_all(self.shared_root(layout)).await?;
        for path in &CONFIG.shared {
            let path = Path::new(path);
            let src = layout.root.join(path);
            if !src.exists() {
                continue;
            }

            let dst = self.shared(layout, path);
            if src.is_dir() {
                self.add_shared_dir(layout, &src, &dst).await?;
            } else {
                self.add_shared_file(&src, &dst).await?;
            }
        }
        Ok(())
    }

    pub async fn cleanup_shared(
        &self,
        layout: &Layout,
    ) -> Result<()> {
        let shared_root = self.shared_root(layout);
        for mount in MountIter::new()? {
            if let Ok(mount) = mount
                && mount.fstype == "fuse"
                && mount.dest.starts_with(&shared_root)
            {
                self.unmount_path(layout, Path::new(&mount.dest)).await?;
            }
        }
        if shared_root.exists() {
            tokio::fs::remove_dir_all(shared_root).await?;
        }
        Ok(())
    }

    pub async fn unmount_agent_overlays(
        &self,
        layout: &Layout,
    ) -> Result<()> {
        for mount in MountIter::new()? {
            if let Ok(mount) = mount
                && mount.fstype == "fuse.fuse-overlayfs"
                && mount.dest.starts_with(layout.agents())
                && mount.dest.ends_with(AGENT_WORKDIR_DIRNAME)
            {
                self.unmount_path(layout, Path::new(&mount.dest)).await?;
            }
        }
        Ok(())
    }

    pub async fn cleanup(
        &self,
        layout: &Layout,
    ) -> Result<()> {
        self.cleanup_shared(layout).await?;
        self.unmount_agent_overlays(layout).await?;
        Ok(())
    }

    async fn mount_status(
        &self,
        layout: &Layout,
        path: &Path,
    ) -> Result<MountStatus> {
        let output = layout
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
        layout: &Layout,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()> {
        match self
            .mount_status(layout, &layout.agent_workdir(aid))
            .await?
        {
            MountStatus::Mounted | MountStatus::Broken => self.unmount(layout, aid).await?,
            MountStatus::Unmounted => (),
        }

        let options = Self::overlay_options(
            &self.snapshot(layout, commit),
            &self.shared_root(layout),
            &self.overlay_upper(layout, aid),
            &self.overlay_workdir(layout, aid),
        );
        let args = [
            "-o".to_string(),
            options,
            layout.agent_workdir(aid).to_string_lossy().to_string(),
        ];
        layout
            .bash("fuse-overlayfs", args)
            .await?
            .status
            .exit_ok()?;
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
        hard_link(src, dst).await?;
        Ok(())
    }

    async fn add_shared_dir(
        &self,
        layout: &Layout,
        src: &Path,
        dst: &Path,
    ) -> Result<()> {
        create_dir_all(dst).await?;
        let args = [
            src.to_string_lossy().to_string(),
            dst.to_string_lossy().to_string(),
            "--no-allow-other".to_string(),
        ];
        layout.bash("bindfs", args).await?.status.exit_ok()?;
        Ok(())
    }

    async fn unmount_path(
        &self,
        layout: &Layout,
        path: &Path,
    ) -> Result<()> {
        layout
            .bash("umount", [path.to_string_lossy().to_string()])
            .await?
            .status
            .exit_ok()?;
        Ok(())
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

    fn overlay(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf {
        layout.agent(aid).join(OVERLAY_DIRNAME)
    }

    fn overlay_workdir(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf {
        self.overlay(layout, aid).join(OVERLAY_WORKDIR_DIRNAME)
    }

    fn overlay_upper(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf {
        self.overlay(layout, aid).join(OVERLAY_UPPER_DIRNAME)
    }

    fn shared(
        &self,
        layout: &Layout,
        path: &Path,
    ) -> PathBuf {
        self.shared_root(layout).join(path)
    }

    // TODO maybe rename these
    fn shared_root(
        &self,
        layout: &Layout,
    ) -> PathBuf {
        layout.data.join(SHARED_DIRNAME)
    }

    fn snapshots(
        &self,
        layout: &Layout,
    ) -> PathBuf {
        layout.data.join(SNAPSHOTS_DIRNAME)
    }

    fn snapshot(
        &self,
        layout: &Layout,
        commit: &str,
    ) -> PathBuf {
        self.snapshots(layout).join(commit)
    }

    pub async fn ensure_snapshot(
        &self,
        layout: &Layout,
        commit: &str,
    ) -> Result<()> {
        use std::process::Command;
        use std::process::Stdio;
        let path = self.snapshot(layout, commit);
        if path.exists() {
            return Ok(());
        }
        create_dir_all(&path).await?;

        let dest = path.clone();
        let root = layout.root.clone();
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
}

impl Layout {
    async fn bash<I, S>(
        &self,
        command: &str,
        args: I,
    ) -> Result<std::process::Output>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        use tokio::process::Command;
        let output = Command::new(command)
            .current_dir(self.root.clone())
            .args(args.into_iter().map(Into::into))
            .output()
            .await?;
        Ok(output)
    }
}

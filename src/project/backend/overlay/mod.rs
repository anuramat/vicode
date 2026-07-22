mod agent;
mod copy;
mod mount_tests;
mod paths;
mod shared;

use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use anyhow::bail;
use thiserror::Error;

use super::Overlay;
use crate::agent::id::AgentId;
use crate::deps;
use crate::project::Paths;
use crate::project::backend::BackendOps;
use crate::sandbox::Sandbox;
use crate::sandbox::SandboxRunner;

enum MountStatus {
    Mounted,
    Unmounted,
    Broken,
}

// TODO move more agent logic to ./agent.rs

impl Overlay {
    fn base_layers(
        &self,
        paths: &Paths,
        commit: &str,
    ) -> Vec<PathBuf> {
        vec![self.snapshot(paths, commit), self.shared(paths)]
    }
}

#[async_trait::async_trait]
impl BackendOps for Overlay {
    fn sandbox_runner(
        &self,
        cwd: PathBuf,
        gitdir: PathBuf,
    ) -> SandboxRunner {
        self.sandbox.runner(cwd, gitdir)
    }

    async fn init(
        &self,
        paths: &Paths,
    ) -> Result<()> {
        self.unmount_all(paths).await?;
        self.init_shared(paths).await?;
        Ok(())
    }

    async fn new_agent_workdir(
        &self,
        paths: &Paths,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()> {
        self.init_overlay(paths, commit, aid).await
    }

    async fn mount_agent(
        &self,
        paths: &Paths,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()> {
        match self.mount_status(paths, &paths.agent_workdir(aid)).await? {
            MountStatus::Mounted => return Ok(()),
            MountStatus::Broken => self.unmount_agent(paths, aid).await?,
            MountStatus::Unmounted => (),
        }

        self.ensure_snapshot(paths, commit).await?;
        let options = Self::overlay_options(
            &self.base_layers(paths, commit),
            &self.overlay_upper(paths, aid),
            &self.overlay_workdir(paths, aid),
        );
        let args = [
            "-o".to_string(),
            options,
            paths.agent_workdir(aid).to_string_lossy().to_string(),
        ];
        let status = paths.run(deps::FUSE_OVERLAYFS, args).await?.status;
        anyhow::ensure!(status.success(), "fuse-overlayfs failed: {status}");
        // mounting invalidates stat cache, so we refresh eagerly
        let workdir = paths.agent_workdir(aid);
        if let Err(error) =
            tokio::task::spawn_blocking(move || crate::git::refresh_index(&workdir)).await?
        {
            tracing::warn!("git index refresh for {aid} failed: {error:#}");
        }
        Ok(())
    }

    async fn unmount_agent(
        &self,
        paths: &Paths,
        aid: &AgentId,
    ) -> Result<()> {
        let path = paths.agent_workdir(aid);
        match self.mount_status(paths, &path).await? {
            MountStatus::Unmounted => Ok(()),
            MountStatus::Mounted | MountStatus::Broken => self.unmount(paths, &path).await,
        }
    }

    async fn unmount_all(
        &self,
        paths: &Paths,
    ) -> Result<()> {
        self.unmount_shared(paths).await?;
        self.unmount_agents(paths).await?;
        Ok(())
    }

    async fn duplicate_agent_workdir(
        &self,
        paths: &Paths,
        src_aid: &AgentId,
        dst_aid: &AgentId,
        commit: &str,
    ) -> Result<()> {
        let src_upper = self.overlay_upper(paths, src_aid);
        let dst_upper = self.overlay_upper(paths, dst_aid);
        let tmp = dst_upper.with_extension("tmp");
        tokio::task::spawn_blocking(move || -> Result<()> {
            copy::copy_layer(&src_upper, &tmp)?;
            std::fs::rename(&tmp, &dst_upper)?;
            Ok(())
        })
        .await??;
        self.init_overlay(paths, commit, dst_aid).await
    }
}

impl Overlay {
    async fn mount_status(
        &self,
        paths: &Paths,
        path: &Path,
    ) -> Result<MountStatus> {
        let output = paths
            .run(deps::MOUNTPOINT, [path.to_string_lossy().to_string()])
            .await?;
        let status = match output.status.code() {
            Some(0) => MountStatus::Mounted,
            Some(1) => MountStatus::Broken,
            Some(32) => MountStatus::Unmounted,
            _ => {
                bail!("unexpected mountpoint output: {output:?}")
            }
        };
        Ok(status)
    }

    async fn unmount(
        &self,
        paths: &Paths,
        path: &Path,
    ) -> Result<()> {
        paths
            .try_run(deps::UMOUNT, [path.to_string_lossy().to_string()])
            .await?;
        Ok(())
    }
}

impl Paths {
    async fn run<I, S>(
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

    async fn try_run<I, S>(
        &self,
        program: &str,
        args: I,
    ) -> Result<()>
    where
        I: IntoIterator<Item = S> + Clone,
        S: Into<String>,
    {
        use tokio::process::Command;
        let output = Command::new(program)
            .current_dir(self.root.clone())
            .args(args.clone().into_iter().map(Into::into))
            .output()
            .await?;
        if !output.status.success() {
            return Err(RunError {
                program: program.to_string(),
                args: args.into_iter().map(Into::into).collect(),
                status: output.status,
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            }
            .into());
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
#[error("command {program} with args {:?} failed with status {}", .args, .status)]
struct RunError {
    program: String,
    args: Vec<String>,
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use similar_asserts::assert_eq;

    use super::*;

    fn rig() -> (Paths, Overlay, String) {
        let root = std::env::temp_dir().join(format!("vicode-overlay-{}", uuid::Uuid::new_v4()));
        let data = root.join(".vicode");
        fs::create_dir_all(&data).unwrap();
        let repo = git2::Repository::init(&root).unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let commit = repo
            .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap()
            .to_string();
        let paths = Paths {
            id: "test".into(),
            root,
            data,
        };
        let overlay = Overlay::test();
        // pre-created snapshot so init_overlay skips the git checkout
        fs::create_dir_all(overlay.snapshot(&paths, &commit)).unwrap();
        (paths, overlay, commit)
    }

    /// each spawn seeds the child's upper from the parent's upper — the
    /// whole inherited delta lives in one layer, with the child's own
    /// worktree pointer on top, and the mount stays constant-depth
    /// ([upper | snapshot | shared]) at any spawn depth
    #[tokio::test]
    async fn duplicate_seeds_the_child_upper_at_constant_depth() {
        let (paths, overlay, commit) = rig();
        let (parent, child, grandchild) = (
            AgentId::from("parent".to_string()),
            AgentId::from("child".to_string()),
            AgentId::from("grandchild".to_string()),
        );
        let parent_upper = overlay.overlay_upper(&paths, &parent);
        fs::create_dir_all(&parent_upper).unwrap();
        fs::write(parent_upper.join("delta.txt"), "parent delta").unwrap();

        overlay
            .duplicate_agent_workdir(&paths, &parent, &child, &commit)
            .await
            .unwrap();

        let child_upper = overlay.overlay_upper(&paths, &child);
        assert_eq!(
            fs::read_to_string(child_upper.join("delta.txt")).unwrap(),
            "parent delta"
        );
        // the .git in the seeded upper is the child's own worktree pointer
        assert!(child_upper.join(".git").is_file());

        // nesting: the grandchild's upper carries the whole inherited delta
        fs::write(child_upper.join("child.txt"), "child delta").unwrap();
        overlay
            .duplicate_agent_workdir(&paths, &child, &grandchild, &commit)
            .await
            .unwrap();
        let gc_upper = overlay.overlay_upper(&paths, &grandchild);
        assert_eq!(
            fs::read_to_string(gc_upper.join("delta.txt")).unwrap(),
            "parent delta"
        );
        assert_eq!(
            fs::read_to_string(gc_upper.join("child.txt")).unwrap(),
            "child delta"
        );
        assert!(!overlay.overlay(&paths, &grandchild).join("lowers").exists());

        // the mount's read-only side is the same two layers at any depth
        let options = Overlay::overlay_options(
            &overlay.base_layers(&paths, &commit),
            &gc_upper,
            &overlay.overlay_workdir(&paths, &grandchild),
        );
        assert_eq!(
            options,
            format!(
                "lowerdir={}:{},upperdir={},workdir={}",
                overlay.snapshot(&paths, &commit).display(),
                overlay.shared(&paths).display(),
                gc_upper.display(),
                overlay.overlay_workdir(&paths, &grandchild).display(),
            )
        );

        fs::remove_dir_all(paths.root).ok();
    }
}

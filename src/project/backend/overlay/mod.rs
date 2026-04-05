mod agent;
mod layout;
mod shared;

use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use anyhow::bail;
use thiserror::Error;

use super::Overlay;
use crate::agent::id::AgentId;
use crate::agent::*;
use crate::deps;
use crate::project::Layout;
use crate::project::backend::Backend;
use crate::project::layout::LayoutTrait;

enum MountStatus {
    Mounted,
    Unmounted,
    Broken,
}

// TODO move more agent logic to ./agent.rs

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
        config: &crate::config::Config,
    ) -> Result<()> {
        self.unmount_all(layout).await?;
        self.init_shared(layout, &config.shared).await?;
        Ok(())
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
        match self
            .mount_status(layout, &layout.agent_workdir(aid))
            .await?
        {
            MountStatus::Mounted | MountStatus::Broken => self.unmount_agent(layout, aid).await?,
            MountStatus::Unmounted => (),
        }

        let options = Self::overlay_options(
            &self.snapshot(layout, commit),
            &self.shared(layout),
            &self.overlay_upper(layout, aid),
            &self.overlay_workdir(layout, aid),
        );
        let args = [
            "-o".to_string(),
            options,
            layout.agent_workdir(aid).to_string_lossy().to_string(),
        ];
        let status = layout.bash(deps::FUSE_OVERLAYFS, args).await?.status;
        anyhow::ensure!(status.success(), "fuse-overlayfs failed: {status}");
        Ok(())
    }

    async fn unmount_agent(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> Result<()> {
        self.unmount(layout, &layout.agent_workdir(aid)).await
    }

    async fn unmount_all(
        &self,
        layout: &Layout,
    ) -> Result<()> {
        self.unmount_shared(layout).await?;
        self.unmount_agents(layout).await?;
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
        let src = self.overlay_upper(layout, src_id);
        let dst = self.overlay_upper(layout, aid);
        crate::git::copy_without_dot_git(&src, dst).await?;
        self.init_overlay(layout, &state.context.commit, aid, git)
            .await?;
        state.save(layout, aid).await
    }
}

impl Overlay {
    async fn mount_status(
        &self,
        layout: &Layout,
        path: &Path,
    ) -> Result<MountStatus> {
        let output = layout
            .bash(deps::MOUNTPOINT, [path.to_string_lossy().to_string()])
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

    async fn unmount(
        &self,
        layout: &Layout,
        path: &Path,
    ) -> Result<()> {
        layout
            .try_bash(deps::UMOUNT, [path.to_string_lossy().to_string()])
            .await?;
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

    async fn try_bash<I, S>(
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
            return Err(BashError {
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
struct BashError {
    program: String,
    args: Vec<String>,
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

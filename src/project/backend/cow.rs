use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use tokio::fs::create_dir_all;

use crate::agent::id::AgentId;
use crate::deps;
use crate::git;
use crate::git::checkout;
use crate::git::worktree;
use crate::project::Layout;
use crate::project::backend::WorkspaceBackend;
use crate::project::layout::LayoutTrait;
use crate::sandbox::Sandbox;
use crate::sandbox::SandboxRunner;

#[async_trait::async_trait]
impl WorkspaceBackend for super::Cow {
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
        layout: &Layout,
        _config: &crate::config::Config,
    ) -> Result<()> {
        // skip Time Machine backups
        let agents = layout.agents();
        tokio::fs::create_dir_all(&agents).await?;
        let path = agents.to_string_lossy();
        match tokio::process::Command::new(deps::TMUTIL)
            .args(["addexclusion", &path])
            .output()
            .await
        {
            Err(e) => tracing::error!("tmutil addexclusion {path}: {e}"),
            Ok(o) if !o.status.success() => tracing::error!(
                "tmutil addexclusion {path}: status={}, stderr={}",
                o.status,
                String::from_utf8_lossy(&o.stderr).trim(),
            ),
            Ok(_) => {}
        }
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
            clone_entries_except_git(from, to).await?;
        } else {
            if let Some(parent) = to.parent() {
                create_dir_all(parent).await?;
            }
            clone_tree(from, to).await?;
        }
        Ok(())
    }
}

async fn clone_tree(
    src: PathBuf,
    dst: PathBuf,
) -> Result<()> {
    tokio::task::spawn_blocking(move || clonefile(&src, &dst)).await?
}

async fn clone_entries_except_git(
    src: PathBuf,
    dst: PathBuf,
) -> Result<()> {
    let mut entries = tokio::fs::read_dir(&src).await?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_name() != ".git" {
            names.push(entry.file_name());
        }
    }
    tokio::task::spawn_blocking(move || -> Result<()> {
        for name in names {
            clonefile(&src.join(&name), &dst.join(&name))?;
        }
        Ok(())
    })
    .await??;
    Ok(())
}

#[cfg(target_os = "macos")]
fn clonefile(
    src: &Path,
    dst: &Path,
) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // don't dereference a top-level symlink
    const CLONE_NOFOLLOW: u32 = 0x0001;

    let src_c = CString::new(src.as_os_str().as_bytes())?;
    let dst_c = CString::new(dst.as_os_str().as_bytes())?;
    let ret = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), CLONE_NOFOLLOW) };
    if ret == -1 {
        let err = std::io::Error::last_os_error();
        anyhow::bail!("clonefile({src:?}, {dst:?}) failed: {err}");
    }
    Ok(())
}

#[cfg(all(target_os = "linux", test))]
fn clonefile(
    src: &Path,
    dst: &Path,
) -> Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    let ft = meta.file_type();
    if ft.is_dir() {
        std::fs::create_dir(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            clonefile(&entry.path(), &dst.join(entry.file_name()))?;
        }
    } else if ft.is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(src)?, dst)?;
    } else {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

#[cfg(all(target_os = "linux", not(test)))]
fn clonefile(
    _src: &Path,
    _dst: &Path,
) -> Result<()> {
    anyhow::bail!("clonefile is only available on macOS")
}

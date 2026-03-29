use std::ffi::CStr;
use std::ffi::CString;
use std::ops::BitOr;
use std::path::Path;
use std::path::PathBuf;
use std::ptr;
use std::str::FromStr;

use anyhow::Result;
use anyhow::bail;
use git2::Repository;
use git2::WorktreeAddOptions;
use libgit2_sys::git_error_last;
use libgit2_sys::{self as raw};
use tokio::fs::create_dir_all;

use crate::agent::AgentId;
use crate::project::Layout;
use crate::project::layout::LayoutTrait;

pub async fn worktree(
    layout: &Layout,
    aid: &AgentId,
    commit: &str,
    checkout: bool,
) -> Result<()> {
    let name = layout.worktree_name(aid);
    let worktree_path = layout.agent_workdir(aid);
    if let Some(parent) = worktree_path.parent() {
        create_dir_all(parent).await?;
    }
    if checkout {
        worktree_with_checkout(&layout.root(), &name, &worktree_path, commit).await
    } else {
        worktree_no_checkout(&layout.root(), &name, &worktree_path, commit).await
    }
}

/// `git worktree add --no-checkout`, but with given worktree name
async fn worktree_no_checkout(
    root: &Path,
    name: &str,
    worktree_path: &Path,
    commit: &str,
) -> Result<()> {
    let repo = Repository::open(root)?;
    let wt_branch = {
        let oid = git2::Oid::from_str(commit)?;
        let target = repo.find_commit(oid)?;
        repo.branch(name, &target, false)?
    };
    let wt_ref = wt_branch.into_reference();

    let name_cstr = CString::from_str(name)?;
    let repo_cstr = CString::from_str(&root.to_string_lossy())?;
    let worktree_cstr = CString::from_str(&worktree_path.to_string_lossy())?;

    unsafe {
        // open the repository
        let mut repo_ptr = ptr::null_mut();
        check(raw::git_repository_open(&mut repo_ptr, repo_cstr.as_ptr()))?;

        // init options with --no-checkout
        let mut opts: raw::git_worktree_add_options = std::mem::zeroed();
        {
            check(raw::git_worktree_add_options_init(
                &mut opts,
                raw::GIT_WORKTREE_ADD_OPTIONS_VERSION,
            ))?;
            // TODO is this line required?
            check(raw::git_checkout_init_options(
                &mut opts.checkout_options,
                raw::GIT_CHECKOUT_OPTIONS_VERSION,
            ))?;
            opts.reference = wt_ref.raw();
            opts.checkout_options.checkout_strategy =
                raw::GIT_CHECKOUT_NONE.bitor(raw::GIT_CHECKOUT_DONT_UPDATE_INDEX);
        }

        // create the worktree
        let mut worktree_ptr = ptr::null_mut();
        check(raw::git_worktree_add(
            &mut worktree_ptr,
            repo_ptr,
            name_cstr.as_ptr(),
            worktree_cstr.as_ptr(),
            &opts,
        ))?;

        if !worktree_ptr.is_null() {
            raw::git_worktree_free(worktree_ptr);
        }

        if !repo_ptr.is_null() {
            raw::git_repository_free(repo_ptr);
        }
    }
    Ok(())
}

async fn worktree_with_checkout(
    root: &Path,
    name: &str,
    worktree_path: &Path,
    commit: &str,
) -> Result<()> {
    let repo = Repository::open(root)?;
    let wt_branch = {
        let oid = git2::Oid::from_str(commit)?;
        let target = repo.find_commit(oid)?;
        repo.branch(name, &target, false)?.into_reference()
    };
    let mut opts = WorktreeAddOptions::new();
    opts.reference(Some(&wt_branch));
    repo.worktree(name, worktree_path, Some(&opts))?;
    Ok(())
}

pub async fn copy_without_dot_git(
    from: &Path,
    to: PathBuf,
) -> Result<()> {
    let items = {
        let mut entries = tokio::fs::read_dir(from).await?;
        let mut items = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_name() != ".git" {
                items.push(entry.path());
            }
        }
        items
    };
    let options = fs_extra::dir::CopyOptions::new().copy_inside(true);
    tokio::task::spawn_blocking(move || fs_extra::copy_items(&items, to, &options)).await??;
    Ok(())
}

unsafe fn check(code: i32) -> Result<()> {
    if code == 0 {
        return Ok(());
    }
    let mut message = None;
    let mut klass = None;
    unsafe {
        // shouldn't be freed: https://libgit2.org/docs/reference/main/errors/git_error_last.html
        let error = git_error_last();
        if let Some(error) = error.as_ref() {
            message = CStr::from_ptr(error.message).to_str().ok();
            klass = Some(error.klass);
        }
    }
    bail!(
        "libgit2 error: code={}, klass={:#?}, message={:#?}",
        code,
        klass,
        message
    );
}

pub async fn checkout(
    layout: &Layout,
    commit: &str,
    path: PathBuf,
) -> Result<()> {
    use std::process::Command;
    use std::process::Stdio;

    let root = layout.root();
    tokio::fs::create_dir_all(&path).await?;

    let dest = path.clone();
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
                    .ok_or_else(|| anyhow::anyhow!("missing git archive stdout"))?,
            )
            .status()?;
        archive.wait()?.exit_ok()?;
        tar.exit_ok()?;
        Ok(())
    })
    .await??;
    Ok(())
}

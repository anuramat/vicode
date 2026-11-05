use std::ffi::CStr;
use std::ffi::CString;
use std::path::Path;
use std::path::PathBuf;
use std::ptr;
use std::str::FromStr;

use anyhow::Result;
use anyhow::bail;
use git2::BranchType;
use git2::ErrorCode;
use git2::Repository;
use git2::StatusOptions;
use git2::WorktreeAddOptions;
use git2::WorktreePruneOptions;
use libgit2_sys::git_error_last;
use libgit2_sys::{self as raw};
use tokio::fs::create_dir_all;
use tracing::warn;

use crate::agent::AgentId;
use crate::deps;
use crate::project::Paths;
use crate::project::paths::worktree_name_to_agent_id;

pub async fn worktree(
    paths: &Paths,
    aid: &AgentId,
    commit: &str,
    checkout: bool,
) -> Result<()> {
    let name = paths.worktree_name(aid);
    let worktree_path = paths.agent_workdir(aid);
    if let Some(parent) = worktree_path.parent() {
        create_dir_all(parent).await?;
    }
    if checkout {
        worktree_with_checkout(paths.root(), &name, &worktree_path, commit)
    } else {
        worktree_no_checkout(paths.root(), &name, &worktree_path, commit)
    }
}

/// `git worktree add --no-checkout`, but with given worktree name
fn worktree_no_checkout(
    root: &Path,
    name: &str,
    worktree_path: &Path,
    commit: &str,
) -> Result<()> {
    let repo = Repository::open(root)?;
    let wt_branch = {
        let oid = git2::Oid::from_str(commit)?;
        let target = repo.find_commit(oid)?;
        repo.branch(name, &target, true)?
    };
    let wt_ref = wt_branch.into_reference();

    let name_cstr = CString::from_str(name)?;
    let repo_cstr = CString::from_str(&root.to_string_lossy())?;
    let worktree_cstr = CString::from_str(&worktree_path.to_string_lossy())?;

    let added = unsafe {
        // open the repository
        let mut repo_ptr = ptr::null_mut();
        let mut worktree_ptr = ptr::null_mut();
        let result = (|| -> Result<()> {
            check(raw::git_repository_open(
                &raw mut repo_ptr,
                repo_cstr.as_ptr(),
            ))?;

            // init options with --no-checkout
            let opts = {
                let mut opts: raw::git_worktree_add_options = std::mem::zeroed();
                check(raw::git_worktree_add_options_init(
                    &raw mut opts,
                    raw::GIT_WORKTREE_ADD_OPTIONS_VERSION,
                ))?;
                opts.reference = wt_ref.raw();
                opts.checkout_options.checkout_strategy = raw::GIT_CHECKOUT_NONE;
                opts
            };

            check(raw::git_worktree_add(
                &raw mut worktree_ptr,
                repo_ptr,
                name_cstr.as_ptr(),
                worktree_cstr.as_ptr(),
                &raw const opts,
            ))
        })();

        if !worktree_ptr.is_null() {
            raw::git_worktree_free(worktree_ptr);
        }
        if !repo_ptr.is_null() {
            raw::git_repository_free(repo_ptr);
        }

        result
    };

    if let Err(error) = added {
        delete_branch_if_exists(&repo, name).ok();
        return Err(error);
    }
    Ok(())
}

fn worktree_with_checkout(
    root: &Path,
    name: &str,
    worktree_path: &Path,
    commit: &str,
) -> Result<()> {
    let repo = Repository::open(root)?;
    let wt_branch = {
        let oid = git2::Oid::from_str(commit)?;
        let target = repo.find_commit(oid)?;
        repo.branch(name, &target, true)?.into_reference()
    };
    let mut opts = WorktreeAddOptions::new();
    opts.reference(Some(&wt_branch));
    if let Err(error) = repo.worktree(name, worktree_path, Some(&opts)) {
        delete_branch_if_exists(&repo, name).ok();
        return Err(error.into());
    }
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
    bail!("libgit2 error: code={code}, klass={klass:#?}, message={message:#?}");
}

// TODO use when we add periodic archive cleanup, the dead-code warning is intentional
pub fn is_workdir_clean(workdir: &Path) -> Result<bool> {
    let repo = Repository::open(workdir)?;
    let mut opts = StatusOptions::new();
    opts.include_ignored(false).include_untracked(true);
    let statuses = repo.statuses(Some(&mut opts))?;
    Ok(statuses.is_empty())
}

pub fn prune_stale_worktrees(paths: &Paths) -> Result<()> {
    let repo = Repository::open(paths.root())?;
    let names = repo.worktrees()?;
    for name in names.iter().flatten() {
        let Some(aid) = worktree_name_to_agent_id(name) else {
            continue;
        };
        if paths.agent(&aid).exists() {
            continue;
        }
        prune_worktree(&repo, name)?;
    }
    Ok(())
}

pub fn prune_worktree(
    repo: &Repository,
    name: &str,
) -> Result<()> {
    let worktree = match repo.find_worktree(name) {
        Ok(w) => w,
        Err(e) if e.code() == ErrorCode::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let mut opts = WorktreePruneOptions::new();
    worktree.prune(Some(&mut opts))?;
    Ok(())
}

fn open_workdir(workdir: &Path) -> Result<Repository> {
    Ok(Repository::open_ext(
        workdir,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&std::ffi::OsStr>(),
    )?)
}

/// warm up the index so our git ops are fast
pub fn refresh_index(workdir: &Path) -> Result<()> {
    let repo = open_workdir(workdir)?;
    let mut opts = git2::DiffOptions::new();
    opts.update_index(true);
    repo.diff_index_to_workdir(None, Some(&mut opts))?;
    repo.index()?.write()?;
    Ok(())
}

/// the full id of the commit `id` (full or abbreviated) names, or of the
/// worktree's own HEAD; branches, tags and rev expressions are rejected
pub fn resolve(
    workdir: &Path,
    id: Option<&str>,
) -> Result<String> {
    let repo = open_workdir(workdir)?;
    let commit = match id {
        Some(id) => repo.find_commit_by_prefix(id)?,
        None => repo.head()?.peel_to_commit()?,
    };
    Ok(commit.id().to_string())
}

/// `git reset --hard`: moves the worktree's branch to `commit` and rewrites
/// only the files that differ
pub fn reset_hard(
    workdir: &Path,
    commit: &str,
) -> Result<()> {
    let repo = open_workdir(workdir)?;
    let target = repo.find_object(git2::Oid::from_str(commit)?, None)?;
    repo.reset(&target, git2::ResetType::Hard, None)?;
    Ok(())
}

pub fn delete_branch_if_exists(
    repo: &Repository,
    branch: &str,
) -> Result<()> {
    match repo.find_branch(branch, BranchType::Local) {
        Ok(mut b) => Ok(b.delete()?),
        Err(e) if e.code() == ErrorCode::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub async fn checkout(
    paths: &Paths,
    commit: &str,
    path: PathBuf,
) -> Result<()> {
    use std::process::Command;
    use std::process::Stdio;

    let root = paths.root().to_path_buf();
    let temp = path.with_extension(format!("tmp{}", std::process::id())); // TODO use a random identifier instead of proces id
    // TODO check that `temp` doesn't exist, error out if it does
    // TODO clean up temps on boot and periodically
    tokio::fs::create_dir_all(&temp).await?;

    let dest = temp.clone();
    let commit = commit.to_string();

    let extracted = tokio::task::spawn_blocking(move || -> Result<()> {
        let mut archive = Command::new(deps::GIT)
            .current_dir(root)
            .args(["archive", &commit])
            .stdout(Stdio::piped())
            .spawn()?;
        let tar = Command::new(deps::TAR)
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
        let archive_status = archive.wait()?;
        anyhow::ensure!(
            archive_status.success(),
            "git archive failed: {archive_status}"
        );
        anyhow::ensure!(tar.success(), "tar failed: {tar}");
        Ok(())
    })
    .await?;
    if extracted.is_err() {
        let removed = tokio::fs::remove_dir_all(&temp).await;
        if let Err(e) = removed {
            warn!("failed to remove temp dir {temp:?}: {e}");
        }
        return extracted;
    }
    if let Err(e) = tokio::fs::rename(&temp, &path).await {
        tokio::fs::remove_dir_all(&temp).await?;
        if !path.exists() {
            // unexpected error
            return Err(e.into());
        }
        // concurrent checkout finished before this one
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use crate::agent::AgentId;
    use crate::project::Project;

    /// only commit ids resolve -- full, abbreviated, or the worktree's HEAD
    /// by default; branch names and rev expressions are rejected
    #[tokio::test]
    async fn resolve_accepts_only_commit_ids() {
        let project = Project::new_test().unwrap().0;
        let aid = AgentId::from("resolver".to_string());
        let head = project.head_commit();
        project.new_agent_workdir(&head, &aid).await.unwrap();
        let wd = project.agent_workdir(&aid);

        assert_eq!(
            [None, Some(head.as_str()), Some(&head[..7])]
                .map(|id| super::resolve(&wd, id).unwrap()),
            [head.clone(), head.clone(), head.clone()]
        );
        let branch = project.worktree_name(&aid);
        for rev in [branch.as_str(), "HEAD", "HEAD~0"] {
            assert!(super::resolve(&wd, Some(rev)).is_err(), "{rev} resolved");
        }
    }
}

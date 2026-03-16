use std::ffi::CStr;
use std::ffi::CString;
use std::ops::BitOr;
use std::path::Path;
use std::ptr;
use std::str::FromStr;

use anyhow::Result;
use anyhow::bail;
use git2::Repository;
use libgit2_sys::git_error_last;
use libgit2_sys::{self as raw};

/// `git worktree add --no-checkout`, but with given worktree name
pub async fn worktree_no_checkout(
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

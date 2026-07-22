use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Context;
use anyhow::Result;
use anyhow::ensure;
use git2::Repository;

use crate::deps;

/// the agent's workdir vs a base commit
pub fn worktree(
    workdir: &Path,
    commit: &str,
    excluded: &[String],
) -> Result<String> {
    // check the commit is valid
    git2::Oid::from_str(commit)?;

    let repo = Repository::open_ext(
        workdir,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&std::ffi::OsStr>(),
    )?;
    let commondir = repo.commondir().to_path_buf();
    let index = {
        let index = repo.index()?;
        index
            .path()
            .context("agent repository has no index")?
            .to_path_buf()
    };

    let scratch = std::env::temp_dir().join(format!("vicode-diff-{}", uuid::Uuid::new_v4()));
    let out = scratch_gitdir(&scratch, &commondir, &index)
        .and_then(|()| render(workdir, &scratch, commit, excluded));
    std::fs::remove_dir_all(&scratch).ok();
    out
}

fn scratch_gitdir(
    scratch: &Path,
    commondir: &Path,
    index: &Path,
) -> Result<()> {
    std::fs::create_dir_all(scratch.join("refs"))?;
    std::fs::write(scratch.join("HEAD"), "ref: refs/heads/scratch\n")?;
    std::fs::write(scratch.join("config"), "")?;
    for shared in ["objects", "info"] {
        std::os::unix::fs::symlink(commondir.join(shared), scratch.join(shared))?;
    }
    std::fs::copy(index, scratch.join("index"))?;
    Ok(())
}

fn git(
    workdir: &Path,
    scratch: &Path,
) -> Command {
    let mut cmd = Command::new(deps::GIT);
    cmd.current_dir(workdir)
        .env("GIT_INDEX_FILE", scratch.join("index"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .arg("--git-dir")
        .arg(scratch)
        .arg("--work-tree")
        .arg(workdir)
        .arg("--no-pager");
    cmd
}

fn drop_commitless_gitlinks(
    workdir: &Path,
    index: &Path,
) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;

    let mut index = git2::Index::open(index)?;
    let unpinnable: Vec<PathBuf> = index
        .iter()
        .filter(|entry| entry.mode == 0o160_000)
        .map(|entry| PathBuf::from(std::ffi::OsStr::from_bytes(&entry.path).to_os_string()))
        .filter(|path| crate::git::nested_head(&workdir.join(path)).is_none())
        .collect();
    if unpinnable.is_empty() {
        return Ok(());
    }
    for path in unpinnable {
        index.remove_path(&path)?;
    }
    index.write()?;
    Ok(())
}

fn render(
    workdir: &Path,
    scratch: &Path,
    commit: &str,
    excluded: &[String],
) -> Result<String> {
    let diff_pathspecs = std::iter::once(".".to_string())
        .chain(
            excluded
                .iter()
                .map(|path| format!(":(top,exclude,literal){path}")),
        )
        .collect::<Vec<_>>();
    let add = git(workdir, scratch)
        .args([
            "-c",
            "advice.addEmbeddedRepo=false",
            "add",
            "--intent-to-add", // to avoid hashing content into the odb
            "--",
            ".",
        ])
        .output()?;
    ensure!(
        add.status.success(),
        "git add -N failed: {}",
        String::from_utf8_lossy(&add.stderr).trim()
    );
    drop_commitless_gitlinks(workdir, &scratch.join("index"))?;
    let diff = git(workdir, scratch)
        .args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
            commit,
            "--",
        ])
        .args(&diff_pathspecs)
        .output()?;
    ensure!(
        diff.status.success(),
        "git diff failed: {}",
        String::from_utf8_lossy(&diff.stderr).trim()
    );

    // might lose some bytes if git gets a false negative on the binary check
    Ok(String::from_utf8_lossy(&diff.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a repo at a fresh temp path with `files` committed on a root commit
    fn repo_with(files: &[(&str, &str)]) -> (PathBuf, Repository, String) {
        let root = std::env::temp_dir().join(format!("vicode-diff-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let repo = Repository::init(&root).unwrap();
        let mut index = repo.index().unwrap();
        for (path, content) in files {
            std::fs::write(root.join(path), content).unwrap();
            index.add_path(Path::new(path)).unwrap();
        }
        index.write().unwrap();
        let commit = {
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let sig = git2::Signature::now("vicode", "vicode@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
                .unwrap()
                .to_string()
        };
        (root, repo, commit)
    }

    /// L6: the primary diff covers edits to tracked files and untracked
    /// files alike, against the given base commit
    #[test]
    fn covers_tracked_edits_and_untracked_files() {
        let (root, _repo, commit) = repo_with(&[("f.txt", "old\n")]);
        std::fs::write(root.join("f.txt"), "new\n").unwrap();
        std::fs::write(root.join("fresh.txt"), "fresh\n").unwrap();
        std::os::unix::fs::symlink("f.txt", root.join("link")).unwrap();

        insta::assert_snapshot!(worktree(&root, &commit, &[]).unwrap(), @r"
        diff --git a/f.txt b/f.txt
        index 3367afd..3e75765 100644
        --- a/f.txt
        +++ b/f.txt
        @@ -1 +1 @@
        -old
        +new
        diff --git a/fresh.txt b/fresh.txt
        new file mode 100644
        index 0000000..92d5444
        --- /dev/null
        +++ b/fresh.txt
        @@ -0,0 +1 @@
        +fresh
        diff --git a/link b/link
        new file mode 120000
        index 0000000..7f66e4f
        --- /dev/null
        +++ b/link
        @@ -0,0 +1 @@
        +f.txt
        \ No newline at end of file
        ");
        std::fs::remove_dir_all(&root).ok();
    }

    /// the scratch index is the whole point: `add -N` must not disturb what
    /// the agent has staged, or an inspect would silently restage its work
    #[test]
    fn leaves_the_agents_own_index_untouched() {
        let (root, repo, commit) = repo_with(&[("f.txt", "old\n")]);
        std::fs::write(root.join("untracked.txt"), "fresh\n").unwrap();
        let index_path = repo.index().unwrap().path().unwrap().to_path_buf();
        let before = std::fs::read(&index_path).unwrap();

        assert!(!worktree(&root, &commit, &[]).unwrap().is_empty());

        similar_asserts::assert_eq!(before, std::fs::read(&index_path).unwrap());
        // and the untracked file is still untracked afterwards
        assert!(
            repo.index()
                .unwrap()
                .get_path(Path::new("untracked.txt"), 0)
                .is_none()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// renames done with a plain `mv` (tracked delete + untracked add) pair
    /// up: exact moves as a bare header, edited moves with hunks
    #[test]
    fn renames_without_git_mv_pair_up() {
        let exact: String = (0..20).map(|i| format!("alpha {i}\n")).collect();
        let edited: String = (0..20).map(|i| format!("beta {i}\n")).collect();
        let (root, _repo, commit) = repo_with(&[("exact.txt", &exact), ("edited.txt", &edited)]);

        std::fs::rename(root.join("exact.txt"), root.join("moved.txt")).unwrap();
        std::fs::rename(root.join("edited.txt"), root.join("renamed.txt")).unwrap();
        std::fs::write(root.join("renamed.txt"), edited.replace("beta 7", "BETA 7")).unwrap();

        insta::assert_snapshot!(worktree(&root, &commit, &[]).unwrap(), @"
        diff --git a/exact.txt b/moved.txt
        similarity index 100%
        rename from exact.txt
        rename to moved.txt
        diff --git a/edited.txt b/renamed.txt
        similarity index 95%
        rename from edited.txt
        rename to renamed.txt
        index 4e1bb32..f733f3a 100644
        --- a/edited.txt
        +++ b/renamed.txt
        @@ -5,7 +5,7 @@ beta 3
         beta 4
         beta 5
         beta 6
        -beta 7
        +BETA 7
         beta 8
         beta 9
         beta 10
        ");
        std::fs::remove_dir_all(&root).ok();
    }

    /// commit everything with a fixed signature, so nested-repo pins are
    /// deterministic and can live in inline snapshots
    fn pin(
        repo: &Repository,
        msg: &str,
        time: i64,
    ) -> git2::Oid {
        let mut idx = repo.index().unwrap();
        idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::new("v", "v@v", &git2::Time::new(time, 0)).unwrap();
        let parents: Vec<_> = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .into_iter()
            .collect();
        let parents: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap()
    }

    /// an embedded repo is a subproject to git: a fresh one renders as a
    /// new 160000 pin, one pinned in the base is silent while untouched, a
    /// HEAD move diffs the two pins, a deletion drops the pin — and a
    /// commitless `git init` is invisible everywhere
    #[test]
    fn nested_repos_render_as_subproject_rows() {
        let (root, repo, base) = repo_with(&[("a.txt", "hello\n")]);
        let sig = git2::Signature::now("vicode", "vicode@example.com").unwrap();

        let vendor = root.join("vendor");
        let inner = Repository::init(&vendor).unwrap();
        std::fs::write(vendor.join("lib.rs"), "one\n").unwrap();
        pin(&inner, "A", 0);
        Repository::init(root.join("empty")).unwrap();

        insta::assert_snapshot!(worktree(&root, &base, &[]).unwrap(), @r"
        diff --git a/vendor b/vendor
        new file mode 160000
        index 0000000..ae50680
        --- /dev/null
        +++ b/vendor
        @@ -0,0 +1 @@
        +Subproject commit ae50680c1872897cbf0483ba1c7e9317ea9ef748
        ");

        // pin it in the base the way the mint does → silent while untouched
        let minted = crate::git::workdir_tree(&root, &[]).unwrap();
        let parent = repo
            .find_commit(git2::Oid::from_str(&base).unwrap())
            .unwrap();
        let based = repo
            .commit(
                None,
                &sig,
                &sig,
                "pinned",
                &repo.find_tree(minted).unwrap(),
                &[&parent],
            )
            .unwrap()
            .to_string();
        insta::assert_snapshot!(worktree(&root, &based, &[]).unwrap(), @"");

        std::fs::write(vendor.join("lib.rs"), "two\n").unwrap();
        pin(&inner, "B", 1);
        insta::assert_snapshot!(worktree(&root, &based, &[]).unwrap(), @r"
        diff --git a/vendor b/vendor
        index ae50680..878909b 160000
        --- a/vendor
        +++ b/vendor
        @@ -1 +1 @@
        -Subproject commit ae50680c1872897cbf0483ba1c7e9317ea9ef748
        +Subproject commit 878909b999abc050218fef773609ea28d8e4dc38
        ");

        std::fs::remove_dir_all(&vendor).unwrap();
        insta::assert_snapshot!(worktree(&root, &based, &[]).unwrap(), @r"
        diff --git a/vendor b/vendor
        deleted file mode 160000
        index ae50680..0000000
        --- a/vendor
        +++ /dev/null
        @@ -1 +0,0 @@
        -Subproject commit ae50680c1872897cbf0483ba1c7e9317ea9ef748
        ");
        std::fs::remove_dir_all(&root).ok();
    }

    /// a chmod with untouched content surfaces as a mode block instead of
    /// comparing equal and vanishing
    #[test]
    fn chmod_renders_mode_block() {
        use std::os::unix::fs::PermissionsExt;

        let (root, _repo, commit) = repo_with(&[("tool.sh", "#!/bin/sh\n")]);
        std::fs::set_permissions(root.join("tool.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();

        insta::assert_snapshot!(worktree(&root, &commit, &[]).unwrap(), @r"
        diff --git a/tool.sh b/tool.sh
        old mode 100644
        new mode 100755
        ");
        std::fs::remove_dir_all(&root).ok();
    }

    /// a file replaced by a symlink is a typechange: a deletion plus an
    /// addition, never a 100644→120000 "chmod" with the two spliced together
    #[test]
    fn typechange_surfaces_as_delete_plus_add() {
        let (root, _repo, commit) = repo_with(&[("x", "target\n")]);
        std::fs::remove_file(root.join("x")).unwrap();
        std::os::unix::fs::symlink("target", root.join("x")).unwrap();

        insta::assert_snapshot!(worktree(&root, &commit, &[]).unwrap(), @r"
        diff --git a/x b/x
        deleted file mode 100644
        index eb5a316..0000000
        --- a/x
        +++ /dev/null
        @@ -1 +0,0 @@
        -target
        diff --git a/x b/x
        new file mode 120000
        index 0000000..1de5659
        --- /dev/null
        +++ b/x
        @@ -0,0 +1 @@
        +target
        \ No newline at end of file
        ");
        std::fs::remove_dir_all(&root).ok();
    }

    /// paths git has to quote (non-UTF-8, control bytes) stay on one header
    /// line, and a binary add carries its mode — both git's own encoding;
    /// a hostile repo-local config must not unquote the paths
    #[test]
    fn quoted_paths_and_binary_adds() {
        let (root, repo, commit) = repo_with(&[("keep.txt", "keep\n")]);
        repo.config()
            .unwrap()
            .set_bool("core.quotepath", false)
            .unwrap();
        std::fs::write(root.join("caf\u{e9}.txt"), "x\n").unwrap();
        std::fs::write(root.join("img.bin"), [0u8, 159]).unwrap();

        insta::assert_snapshot!(worktree(&root, &commit, &[]).unwrap(), @r#"
        diff --git "a/caf\303\251.txt" "b/caf\303\251.txt"
        new file mode 100644
        index 0000000..587be6b
        --- /dev/null
        +++ "b/caf\303\251.txt"
        @@ -0,0 +1 @@
        +x
        diff --git a/img.bin b/img.bin
        new file mode 100644
        index 0000000..b1162a9
        Binary files /dev/null and b/img.bin differ
        "#);
        std::fs::remove_dir_all(&root).ok();
    }

    /// .git/config is agent-writable and inspect runs git on the host, so
    /// keys that make git exec commands (core.fsmonitor, clean filters)
    /// must never fire: local config is ignored wholesale
    #[test]
    fn repo_local_config_never_runs_commands() {
        use std::os::unix::fs::PermissionsExt;

        let (root, repo, commit) = repo_with(&[("f.txt", "old\n")]);
        let canary = root.join("canary");
        let hook = root.join("hook.sh");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", canary.display())).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut cfg = repo.config().unwrap();
        let hook = hook.to_str().unwrap();
        cfg.set_str("core.fsmonitor", hook).unwrap();
        cfg.set_str("filter.evil.clean", hook).unwrap();
        cfg.set_bool("diff.noprefix", true).unwrap();
        std::fs::write(root.join(".gitattributes"), "* filter=evil\n").unwrap();
        std::fs::write(root.join("f.txt"), "new\n").unwrap();

        let out = worktree(&root, &commit, &[]).unwrap();
        assert!(out.contains("--- a/f.txt"), "{out}");
        assert!(!canary.exists());
        std::fs::remove_dir_all(&root).ok();
    }

    /// configured shared paths stay out even if the agent force-added one:
    /// gitignore is a useful default, not the isolation boundary
    #[test]
    fn excluded_paths_stay_out_after_force_add() {
        let (root, repo, commit) = repo_with(&[(".gitignore", "build/\n")]);
        std::fs::create_dir(root.join("build")).unwrap();
        std::fs::write(root.join("build/out.o"), "junk\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("build/out.o")).unwrap();
        index.write().unwrap();

        assert!(!worktree(&root, &commit, &[]).unwrap().is_empty());
        insta::assert_snapshot!(
            worktree(&root, &commit, &["build".into()]).unwrap(),
            @""
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// exclusion must hold for a path tracked in the base too — index
    /// surgery alone would render it as a phantom deletion; the diff
    /// pathspec is what carries it
    #[test]
    fn excluded_paths_tracked_in_base_stay_out() {
        let (root, repo, _) = repo_with(&[("f.txt", "one\n")]);
        std::fs::create_dir(root.join("build")).unwrap();
        std::fs::write(root.join("build/out.o"), "junk\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("build/out.o")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let commit = repo
            .commit(Some("HEAD"), &sig, &sig, "track", &tree, &[&head])
            .unwrap()
            .to_string();
        std::fs::write(root.join("build/out.o"), "changed\n").unwrap();

        assert!(!worktree(&root, &commit, &[]).unwrap().is_empty());
        insta::assert_snapshot!(worktree(&root, &commit, &["build".into()]).unwrap(), @"");
        std::fs::remove_dir_all(&root).ok();
    }
}

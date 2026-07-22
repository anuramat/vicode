use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;

use crate::agent::id::AgentId;
use crate::deps;
use crate::git::worktree;
use crate::project::Paths;
use crate::project::backend::BackendOps;
use crate::sandbox::Sandbox;
use crate::sandbox::SandboxRunner;

#[async_trait::async_trait]
impl BackendOps for super::Cow {
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
        // skip Time Machine backups
        let agents = paths.agents();
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
        paths: &Paths,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()> {
        // TODO wouldn't it be better to do no-checkout and then do cow on the files?
        worktree(paths, aid, commit, true).await
    }

    async fn mount_agent(
        &self,
        _paths: &Paths,
        _commit: &str,
        _aid: &AgentId,
    ) -> Result<()> {
        Ok(())
    }

    async fn unmount_agent(
        &self,
        _paths: &Paths,
        _aid: &AgentId,
    ) -> Result<()> {
        Ok(())
    }

    async fn unmount_all(
        &self,
        _paths: &Paths,
    ) -> Result<()> {
        Ok(())
    }

    async fn duplicate_agent_workdir(
        &self,
        paths: &Paths,
        src_aid: &AgentId,
        dst_aid: &AgentId,
        commit: &str,
    ) -> Result<()> {
        let from = paths.agent_workdir(src_aid);
        let to = paths.agent_workdir(dst_aid);
        worktree(paths, dst_aid, commit, false).await?;
        clone_entries_except_git(from, to).await?;
        let workdir = paths.agent_workdir(dst_aid);
        let oid = git2::Oid::from_str(commit)?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let repo = git2::Repository::open(&workdir)?;
            repo.reset(&repo.find_object(oid, None)?, git2::ResetType::Mixed, None)?;
            crate::git::refresh_index(&workdir)
        })
        .await??;
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use similar_asserts::assert_eq;

    use crate::agent::id::AgentId;
    use crate::project::Project;

    /// write files into the project root and commit them on HEAD
    fn commit_files(
        project: &Project,
        files: &[(&str, &str)],
    ) -> String {
        let repo = git2::Repository::open(project.root()).unwrap();
        let mut index = repo.index().unwrap();
        for (path, content) in files {
            std::fs::write(project.root().join(path), content).unwrap();
            index.add_path(Path::new(path)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("vicode", "vicode@example.com").unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "content", &tree, &[&head])
            .unwrap()
            .to_string()
    }

    fn commit_tree(
        project: &Project,
        commit: &str,
    ) -> git2::Oid {
        git2::Repository::open(project.root())
            .unwrap()
            .find_commit(git2::Oid::from_str(commit).unwrap())
            .unwrap()
            .tree_id()
    }

    /// a duplicate's index must end up populated and stat-warm.
    /// `--no-checkout` leaves it empty, which showed an untouched copy as a
    /// whole tree of staged deletions in the agent's own `git status`; a
    /// cold one makes every git call — ours and the agent's — rehash the
    /// entire workdir
    #[tokio::test]
    async fn duplicate_leaves_a_populated_warm_index() {
        let project = Project::new_test().unwrap().0;
        let commit = commit_files(&project, &[("a.txt", "a\n"), ("b.txt", "b\n")]);
        let parent = AgentId::from("cow-idx-parent".to_string());
        let child = AgentId::from("cow-idx-child".to_string());
        project.new_agent_workdir(&commit, &parent).await.unwrap();
        project
            .duplicate_agent_workdir(&parent, &child, &commit)
            .await
            .unwrap();

        let repo = git2::Repository::open(project.agent_workdir(&child)).unwrap();
        let index = repo.index().unwrap();
        assert!(index.get_path(Path::new("a.txt"), 0).is_some());
        assert_ne!(index.get(0).unwrap().ino, 0, "index carries no stat cache");
        // an untouched duplicate is clean
        assert_eq!(repo.statuses(None).unwrap().len(), 0);
    }

    /// the minted tree covers adds, mods and deletes in the workdir
    #[tokio::test]
    async fn workdir_tree_tracks_adds_mods_and_deletes() {
        let project = Project::new_test().unwrap().0;
        let commit = commit_files(&project, &[("changed.txt", "old\n"), ("gone.txt", "bye\n")]);
        let aid = AgentId::from("cow-tree".to_string());
        project.new_agent_workdir(&commit, &aid).await.unwrap();
        let wd = project.agent_workdir(&aid);
        std::fs::write(wd.join("changed.txt"), "new\n").unwrap();
        std::fs::remove_file(wd.join("gone.txt")).unwrap();
        std::fs::write(wd.join("fresh.txt"), "fresh\n").unwrap();

        let tree = crate::git::workdir_tree(&wd, &[]).unwrap();
        let repo = git2::Repository::open(project.root()).unwrap();
        let names: Vec<String> = repo
            .find_tree(tree)
            .unwrap()
            .iter()
            .map(|e| e.name().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["changed.txt", "fresh.txt"]);
        assert_ne!(tree, commit_tree(&project, &commit));
    }

    /// shared-lower exclusions are structural: a force-added ignored secret
    /// cannot enter a minted spawn-base tree
    #[tokio::test]
    async fn workdir_tree_excludes_force_added_paths() {
        let project = Project::new_test().unwrap().0;
        let commit = commit_files(&project, &[(".gitignore", ".env\n")]);
        let aid = AgentId::from("cow-excluded".to_string());
        project.new_agent_workdir(&commit, &aid).await.unwrap();
        let wd = project.agent_workdir(&aid);
        std::fs::write(wd.join(".env"), "SECRET=1\n").unwrap();
        let repo = git2::Repository::open(&wd).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(".env")).unwrap();
        index.write().unwrap();

        let tree = repo
            .find_tree(crate::git::workdir_tree(&wd, &[".env".into()]).unwrap())
            .unwrap();
        assert!(tree.get_name(".env").is_none());
        assert!(tree.get_name(".gitignore").is_some());
    }

    /// a mid-merge workdir mints fine: `add_all` collapses conflict stages
    /// to the working copy — markers and all — like `git add -A`, and the
    /// agent's own merge state stays untouched
    #[tokio::test]
    async fn workdir_tree_survives_a_conflicted_index() {
        let project = Project::new_test().unwrap().0;
        let commit = commit_files(&project, &[("f.txt", "base\n")]);
        let aid = AgentId::from("cow-conflict".to_string());
        project.new_agent_workdir(&commit, &aid).await.unwrap();
        let wd = project.agent_workdir(&aid);

        let repo = git2::Repository::open(&wd).unwrap();
        let sig = git2::Signature::now("v", "v@v").unwrap();
        // ours: an edit committed on the agent branch
        std::fs::write(wd.join("f.txt"), "ours\n").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(Path::new("f.txt")).unwrap();
        idx.write().unwrap();
        let ours_tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "ours", &ours_tree, &[&parent])
            .unwrap();
        // theirs: a sibling commit off the base, merged in to conflict
        let blob = repo.blob(b"theirs\n").unwrap();
        let mut builder = repo.treebuilder(None).unwrap();
        builder.insert("f.txt", blob, 0o100644).unwrap();
        let their_tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let base = repo
            .find_commit(git2::Oid::from_str(&commit).unwrap())
            .unwrap();
        let their = repo
            .commit(None, &sig, &sig, "theirs", &their_tree, &[&base])
            .unwrap();
        repo.merge(&[&repo.find_annotated_commit(their).unwrap()], None, None)
            .unwrap();
        assert!(repo.index().unwrap().has_conflicts());

        let minted = repo
            .find_tree(crate::git::workdir_tree(&wd, &[]).unwrap())
            .unwrap();
        let content = repo
            .find_blob(minted.get_name("f.txt").unwrap().id())
            .unwrap();
        assert_eq!(
            std::str::from_utf8(content.content()).unwrap(),
            std::fs::read_to_string(wd.join("f.txt")).unwrap()
        );
        assert!(repo.index().unwrap().has_conflicts());
    }

    /// a symlink pointing at a repo is recorded as a symlink, not
    /// misclassified as an embedded repo and pinned as a gitlink
    #[tokio::test]
    async fn workdir_tree_keeps_repo_symlink_a_symlink() {
        let project = Project::new_test().unwrap().0;
        let commit = commit_files(&project, &[("a.txt", "a\n")]);
        let aid = AgentId::from("cow-symlink".to_string());
        project.new_agent_workdir(&commit, &aid).await.unwrap();
        let wd = project.agent_workdir(&aid);
        git2::Repository::init(wd.join("vendor")).unwrap();
        std::os::unix::fs::symlink("vendor", wd.join("link")).unwrap();

        let repo = git2::Repository::open(project.root()).unwrap();
        let tree = repo
            .find_tree(crate::git::workdir_tree(&wd, &[]).unwrap())
            .unwrap();
        assert_eq!(tree.get_name("link").unwrap().filemode(), 0o120000);
        // the commitless vendor repo itself stays out, as documented
        assert!(tree.get_name("vendor").is_none());
    }

    /// F2 regression: a tracked file matching gitignore must not vanish from
    /// the minted tree (`add_all` never sees it — the seed does), and its
    /// edits still surface in the diff (tracked wins)
    #[tokio::test]
    async fn ignored_but_tracked_files_are_retained() {
        let project = Project::new_test().unwrap().0;
        let commit = commit_files(
            &project,
            &[(".gitignore", ".env\n"), (".env", "SECRET=1\n")],
        );
        let aid = AgentId::from("cow-env".to_string());
        project.new_agent_workdir(&commit, &aid).await.unwrap();
        let wd = project.agent_workdir(&aid);

        // an untouched workdir mints back the base tree exactly
        assert_eq!(
            crate::git::workdir_tree(&wd, &[]).unwrap(),
            commit_tree(&project, &commit)
        );

        std::fs::write(wd.join(".env"), "SECRET=2\n").unwrap();
        insta::assert_snapshot!(crate::diff::worktree(&wd, &commit, &[]).unwrap(), @"
        diff --git a/.env b/.env
        index 65ec267..2049a18 100644
        --- a/.env
        +++ b/.env
        @@ -1 +1 @@
        -SECRET=1
        +SECRET=2
        ");
    }

    /// L3: the duplicate skips the parent's top-level .git (a shared gitdir
    /// would let a child commit move the parent's branch) — the child gets
    /// its own worktree pointer instead
    #[tokio::test]
    async fn duplicate_gives_own_worktree() {
        let project = Project::new_test().unwrap().0;
        let commit = project.head_commit();
        let parent = AgentId::from("cow-parent".to_string());
        let child = AgentId::from("cow-child".to_string());
        let parent_workdir = project.agent_workdir(&parent);
        std::fs::create_dir_all(parent_workdir.join(".git")).unwrap();
        std::fs::write(parent_workdir.join(".git/HEAD"), "ref: parent").unwrap();
        std::fs::write(parent_workdir.join("work.txt"), "content").unwrap();

        project
            .duplicate_agent_workdir(&parent, &child, &commit)
            .await
            .unwrap();

        let child_workdir = project.agent_workdir(&child);
        similar_asserts::assert_eq!(
            std::fs::read_to_string(child_workdir.join("work.txt")).unwrap(),
            "content"
        );
        // the child's .git is its own worktree pointer, not the parent's dir
        assert!(child_workdir.join(".git").is_file());
    }
}

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;

use anyhow::Result;
use git2::Repository;
use tokio::fs::create_dir_all;

use super::Overlay;
use crate::agent::id::AgentId;
use crate::git::checkout;
use crate::git::worktree;
use crate::project::Paths;
use crate::project::paths::AGENT_WORKDIR_DIRNAME;

/// to avoid snapshot write race
static SNAPSHOT_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

impl Overlay {
    pub async fn init_overlay(
        &self,
        paths: &Paths,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()> {
        create_dir_all(self.overlay_workdir(paths, aid)).await?;
        create_dir_all(self.overlay_upper(paths, aid)).await?;
        self.ensure_snapshot(paths, commit).await?;

        worktree(paths, aid, commit, false).await?;
        // mixed reset only writes HEAD and the index, so it works before the
        // overlay is mounted, while the worktree is still empty
        {
            let repo = Repository::open(paths.agent_workdir(aid))?;
            let oid = git2::Oid::from_str(commit)?;
            let target = repo.find_object(oid, None)?;
            repo.reset(&target, git2::ResetType::Mixed, None)?;
        }
        tokio::fs::rename(
            paths.agent_workdir(aid).join(".git"),
            self.overlay_upper(paths, aid).join(".git"),
        )
        .await?;

        Ok(())
    }

    pub async fn ensure_snapshot(
        &self,
        paths: &Paths,
        commit: &str,
    ) -> Result<()> {
        let path = self.snapshot(paths, commit);
        if path.exists() {
            return Ok(());
        }
        let lock = Arc::clone(
            SNAPSHOT_LOCKS
                .lock()
                .expect("snapshot locks poisoned")
                .entry(path.clone())
                .or_default(),
        );
        let _guard = lock.lock().await;
        if path.exists() {
            return Ok(());
        }
        checkout(paths, commit, path).await
    }

    pub fn overlay_options(
        lowers: &[std::path::PathBuf],
        upper: &Path,
        workdir: &Path,
    ) -> String {
        let lowerdir = lowers
            .iter()
            .map(|p| escape_mount_path(p))
            .collect::<Vec<_>>()
            .join(":");
        format!(
            "lowerdir={lowerdir},upperdir={},workdir={}",
            escape_mount_path(upper),
            escape_mount_path(workdir),
        )
    }

    pub async fn unmount_agents(
        &self,
        paths: &Paths,
    ) -> Result<()> {
        for mount in proc_mounts::MountIter::new()? {
            if let Ok(mount) = mount
                && mount.fstype == "fuse.fuse-overlayfs"
                && mount.dest.starts_with(paths.agents())
                && mount.dest.ends_with(AGENT_WORKDIR_DIRNAME)
            {
                self.unmount(paths, Path::new(&mount.dest)).await?;
            }
        }
        Ok(())
    }
}

fn escape_mount_path(p: &Path) -> String {
    // TODO is lossy safe here?
    p.to_string_lossy()
        .replace('\\', r"\\")
        .replace(':', r"\:")
        .replace(',', r"\,")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use similar_asserts::assert_eq;

    use super::*;

    fn git_rig() -> (Paths, Overlay, String) {
        let root = std::env::temp_dir().join(format!("vicode-snap-{}", uuid::Uuid::new_v4()));
        let data = root.join(".vicode");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(root.join("f.txt"), "content").unwrap();
        let repo = Repository::init(&root).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("f.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
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
        (paths, overlay, commit)
    }

    /// H7: a failed extraction never occupies the snapshot path — so
    /// `ensure_snapshot` retries instead of forever mounting a partial copy
    #[tokio::test]
    async fn failed_checkout_leaves_no_snapshot_and_retry_succeeds() {
        let (paths, overlay, commit) = git_rig();
        let bad = overlay.snapshot(&paths, "deadbeef");
        assert!(checkout(&paths, "deadbeef", bad.clone()).await.is_err());
        assert!(!bad.exists());

        overlay.ensure_snapshot(&paths, &commit).await.unwrap();
        let snap = overlay.snapshot(&paths, &commit);
        assert_eq!(
            std::fs::read_to_string(snap.join("f.txt")).unwrap(),
            "content"
        );
        std::fs::remove_dir_all(paths.root()).ok();
    }

    /// H7: concurrent `ensure_snapshot`s of one commit singleflight — every
    /// caller returns only once a complete snapshot is in place
    #[tokio::test]
    async fn concurrent_ensure_snapshot_yields_one_complete_snapshot() {
        let (paths, overlay, commit) = git_rig();
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let (paths, overlay, commit) = (paths.clone(), overlay.clone(), commit.clone());
                tokio::spawn(async move { overlay.ensure_snapshot(&paths, &commit).await })
            })
            .collect();
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        let snap = overlay.snapshot(&paths, &commit);
        assert_eq!(
            std::fs::read_to_string(snap.join("f.txt")).unwrap(),
            "content"
        );
        std::fs::remove_dir_all(paths.root()).ok();
    }

    #[test]
    fn overlay_options_escapes_separators() {
        let opts = Overlay::overlay_options(
            &[PathBuf::from("/a:b"), PathBuf::from("/c,d")],
            Path::new("/up,per"),
            Path::new("/work:dir"),
        );
        assert_eq!(
            opts,
            r"lowerdir=/a\:b:/c\,d,upperdir=/up\,per,workdir=/work\:dir"
        );
    }
}

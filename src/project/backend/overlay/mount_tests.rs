//! real-mount overlay tests: gated on fuse availability — they skip
//! where `/dev/fuse` or the binary is missing (macOS, minimal CI, sandboxes)
//! — but a mount that works with the wrong whiteout representation fails
//! loudly instead of passing vacuously
#![cfg(test)]

use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use git2::Repository;
use similar_asserts::assert_eq;

use crate::agent::AgentId;
use crate::deps;
use crate::project::Paths;
use crate::project::Project;
use crate::project::backend::Overlay;

/// real mounts need the fuse-overlayfs binary and /dev/fuse
fn fuse_available() -> bool {
    Path::new("/dev/fuse").exists()
        && std::process::Command::new(deps::FUSE_OVERLAYFS)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
}

fn commit_files(
    project: &Project,
    paths: &[&str],
) -> String {
    let repo = Repository::open(project.root()).unwrap();
    let mut index = repo.index().unwrap();
    for path in paths {
        index.add_path(Path::new(path)).unwrap();
    }
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("vicode", "vicode@example.com").unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "content", &tree, &[&head])
        .unwrap()
        .to_string()
}

/// unmounts and deletes the project on drop, so a failing assertion cannot
/// strand FUSE mounts on the developer's machine — cleanup used to be the
/// last statement of each test body, which every panic skipped
pub struct Rig {
    pub project: Project,
    pub paths: Paths,
    pub overlay: Overlay,
    pub commit: String,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let agents = self.paths.agents();
        for mount in proc_mounts::MountIter::new()
            .into_iter()
            .flatten()
            .flatten()
        {
            if mount.fstype == "fuse.fuse-overlayfs" && mount.dest.starts_with(&agents) {
                drop(
                    std::process::Command::new(deps::UMOUNT)
                        .arg(&mount.dest)
                        .status(),
                );
            }
        }
        fs::remove_dir_all(self.project.root()).ok();
    }
}

/// overlay project with committed content; the shared lower must exist for
/// mounts even though these tests never run `init`
fn harness() -> Rig {
    let project = Project::new_test_overlay().unwrap().0;
    fs::write(project.root().join("base.txt"), "base v1\n").unwrap();
    fs::write(project.root().join("doomed.txt"), "delete me\n").unwrap();
    fs::create_dir(project.root().join("dir")).unwrap();
    fs::write(project.root().join("dir/a.txt"), "a\n").unwrap();
    fs::write(project.root().join("dir/b.txt"), "b\n").unwrap();
    let commit = commit_files(
        &project,
        &["base.txt", "doomed.txt", "dir/a.txt", "dir/b.txt"],
    );
    let paths = Paths {
        root: project.root().to_path_buf(),
        id: project.id().into(),
        data: project.data().to_path_buf(),
    };
    let overlay = Overlay::test();
    fs::create_dir_all(overlay.shared(&paths)).unwrap();
    Rig {
        project,
        paths,
        overlay,
        commit,
    }
}

/// a chain of tab duplicates composes every copied delta under a real
/// mount, deletions included; and the whiteout representation is pinned to
/// what the layer copy assumes
#[tokio::test]
async fn duplicate_chains_compose_deltas_and_preserve_whiteouts() {
    if !fuse_available() {
        eprintln!("skipping: fuse-overlayfs unavailable");
        return;
    }
    let rig = harness();
    let (project, paths, overlay, commit) =
        (&rig.project, &rig.paths, &rig.overlay, rig.commit.clone());
    let gp = AgentId::from("gp".to_string());
    let mid = AgentId::from("mid".to_string());
    let leaf = AgentId::from("leaf".to_string());

    // grandparent: a mounted primary edits its tree
    project.new_agent_workdir(&commit, &gp).await.unwrap();
    project.mount_agent(&commit, &gp).await.unwrap();
    let gp_wd = project.agent_workdir(&gp);
    assert_eq!(
        fs::read_to_string(gp_wd.join("base.txt")).unwrap(),
        "base v1\n"
    );
    fs::write(gp_wd.join("base.txt"), "base v2\n").unwrap();
    fs::write(gp_wd.join("gp.txt"), "gp\n").unwrap();
    fs::remove_file(gp_wd.join("doomed.txt")).unwrap();

    // pin the whiteout representation: the layer copy (`copy.rs`) assumes
    // char-device `0:0` whiteouts, and fuse-overlayfs silently falls back
    // to xattr-marked regular files where mknod is denied — a mode in which
    // copies would silently lose deletions, so fail loudly instead of
    // letting the suite pass vacuously
    let meta = fs::symlink_metadata(overlay.overlay_upper(&paths, &gp).join("doomed.txt"))
        .expect("no whiteout in the upper layer for a file deleted through the mount");
    assert!(
        meta.file_type().is_char_device() && meta.rdev() == 0,
        "fuse-overlayfs produced a non-char-device whiteout (xattr fallback?): \
         layer copies would silently lose deletions"
    );

    // duplicate: the copied delta composes under a real mount, whiteouts
    // included — the grandparent's deletion stays deleted
    project
        .duplicate_agent_workdir(&gp, &mid, &commit)
        .await
        .unwrap();
    project.mount_agent(&commit, &mid).await.unwrap();
    let mid_wd = project.agent_workdir(&mid);
    assert_eq!(
        fs::read_to_string(mid_wd.join("base.txt")).unwrap(),
        "base v2\n"
    );
    assert_eq!(fs::read_to_string(mid_wd.join("gp.txt")).unwrap(), "gp\n");
    assert!(!mid_wd.join("doomed.txt").exists());
    fs::write(mid_wd.join("mid.txt"), "mid\n").unwrap();
    fs::remove_file(mid_wd.join("gp.txt")).unwrap();

    // a duplicate of the duplicate sees the whole chain
    project
        .duplicate_agent_workdir(&mid, &leaf, &commit)
        .await
        .unwrap();
    project.mount_agent(&commit, &leaf).await.unwrap();
    let leaf_wd = project.agent_workdir(&leaf);
    assert_eq!(
        fs::read_to_string(leaf_wd.join("base.txt")).unwrap(),
        "base v2\n"
    );
    assert_eq!(
        fs::read_to_string(leaf_wd.join("mid.txt")).unwrap(),
        "mid\n"
    );
    assert!(!leaf_wd.join("gp.txt").exists());
    assert!(!leaf_wd.join("doomed.txt").exists());
}

/// a spawned workdir shares the tab's snapshot and reaches its start commit
/// by a hard reset through the mount: the composed view matches the commit
/// (a deleted directory included), the upper holds only the delta — the
/// directory as a whiteout, not a copy — and a remount keeps it
#[tokio::test]
async fn spawn_resets_through_the_mount() {
    if !fuse_available() {
        eprintln!("skipping: fuse-overlayfs unavailable");
        return;
    }
    let rig = harness();
    let (project, paths, overlay, snapshot) =
        (&rig.project, &rig.paths, &rig.overlay, rig.commit.clone());
    let root = project.root();
    fs::write(root.join("base.txt"), "base v2\n").unwrap();
    fs::write(root.join("new.txt"), "new\n").unwrap();
    fs::remove_file(root.join("doomed.txt")).unwrap();
    fs::remove_dir_all(root.join("dir")).unwrap();
    let start = {
        let repo = Repository::open(root).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.update_all(["*"], None).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("vicode", "vicode@example.com").unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "start", &tree, &[&head])
            .unwrap()
            .to_string()
    };

    let kid = AgentId::from("kid".to_string());
    project
        .spawn_agent_workdir(&kid, &snapshot, &start)
        .await
        .unwrap();

    let wd = project.agent_workdir(&kid);
    let check = || {
        assert_eq!(
            fs::read_to_string(wd.join("base.txt")).unwrap(),
            "base v2\n"
        );
        assert_eq!(fs::read_to_string(wd.join("new.txt")).unwrap(), "new\n");
        assert!(!wd.join("doomed.txt").exists());
        assert!(!wd.join("dir").exists());
        let repo = Repository::open(&wd).unwrap();
        assert_eq!(repo.head().unwrap().target().unwrap().to_string(), start);
        assert!(repo.statuses(None).unwrap().is_empty());
    };
    check();
    let dir = fs::symlink_metadata(overlay.overlay_upper(&paths, &kid).join("dir")).unwrap();
    assert!(dir.file_type().is_char_device(), "{dir:?}");

    project.unmount_agent(&kid).await.unwrap();
    project.mount_agent(&snapshot, &kid).await.unwrap();
    check();
}

/// Durability: unmount/restart remount preserves the delta in the upper
/// layer.
#[tokio::test]
async fn remount_preserves_delta() {
    if !fuse_available() {
        eprintln!("skipping: fuse-overlayfs unavailable");
        return;
    }
    let rig = harness();
    let (project, commit) = (&rig.project, rig.commit.clone());
    let aid = AgentId::from("solo".to_string());
    project.new_agent_workdir(&commit, &aid).await.unwrap();
    project.mount_agent(&commit, &aid).await.unwrap();
    let wd = project.agent_workdir(&aid);
    fs::write(wd.join("work.txt"), "progress\n").unwrap();
    fs::remove_file(wd.join("doomed.txt")).unwrap();

    project.unmount_agent(&aid).await.unwrap();
    // the raw dir is empty between mounts: the delta lives in the upper
    assert!(!wd.join("work.txt").exists());

    project.mount_agent(&commit, &aid).await.unwrap();
    assert_eq!(
        fs::read_to_string(wd.join("work.txt")).unwrap(),
        "progress\n"
    );
    assert!(!wd.join("doomed.txt").exists());
}

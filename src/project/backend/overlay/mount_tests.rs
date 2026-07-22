//! real-mount overlay tests (§6): gated on fuse availability — they skip
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

/// the inspect diff: the target's mounted workdir vs its frozen base commit
fn inspect_diff(
    project: &Project,
    target: &AgentId,
    base: &str,
) -> String {
    crate::diff::worktree(&project.agent_workdir(target), base, &[]).unwrap()
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

/// §6: a grandchild's mounted view includes grandparent + parent deltas —
/// deletions included — while its own diff shows only its own edits; and
/// the whiteout representation is pinned to what the layer copy and the
/// diff reader assume
#[tokio::test]
async fn nested_mounts_compose_deltas_and_preserve_whiteouts() {
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

    // pin the whiteout representation: the layer copy (`copy.rs`) and the
    // diff reader (`diff.rs`) assume char-device `0:0` whiteouts, and
    // fuse-overlayfs silently falls back to xattr-marked regular files
    // where mknod is denied — a mode in which both would silently miss
    // deletions, so fail loudly instead of letting the suite pass vacuously
    let meta = fs::symlink_metadata(overlay.overlay_upper(&paths, &gp).join("doomed.txt"))
        .expect("no whiteout in the upper layer for a file deleted through the mount");
    assert!(
        meta.file_type().is_char_device() && meta.rdev() == 0,
        "fuse-overlayfs produced a non-char-device whiteout (xattr fallback?): \
         layer copies and diffs would silently lose deletions"
    );

    // child: the copied delta chain composes under a real mount, whiteouts
    // included — the grandparent's deletion stays deleted; the mint (which
    // the store-less rig otherwise bypasses) freezes the spawn base
    project
        .duplicate_agent_workdir(&gp, &mid, &commit)
        .await
        .unwrap();
    let mid_base = project
        .mint_spawn_base(&mid, &commit, &commit)
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

    // grandchild: sees the whole chain; its diff starts empty and picks up
    // only its own edits
    project
        .duplicate_agent_workdir(&mid, &leaf, &commit)
        .await
        .unwrap();
    let leaf_base = project
        .mint_spawn_base(&leaf, &mid_base, &commit)
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
    assert_eq!(inspect_diff(&project, &leaf, &leaf_base), "");

    fs::write(leaf_wd.join("leaf.txt"), "leaf\n").unwrap();
    fs::remove_file(leaf_wd.join("mid.txt")).unwrap();
    // mid.txt lives only in the leaf's seeded upper — no lower holds it, so
    // its deletion leaves no whiteout behind. The mount is the ground truth:
    // the file is simply gone from the composed view, so it renders deleted
    insta::assert_snapshot!(inspect_diff(&project, &leaf, &leaf_base), @"
    diff --git a/leaf.txt b/leaf.txt
    new file mode 100644
    index 0000000..9a07dce
    --- /dev/null
    +++ b/leaf.txt
    @@ -0,0 +1 @@
    +leaf
    diff --git a/mid.txt b/mid.txt
    deleted file mode 100644
    index 987fcca..0000000
    --- a/mid.txt
    +++ /dev/null
    @@ -1 +0,0 @@
    -mid
    ");
}

/// M7: `rm -rf dir && mkdir dir && echo > dir/new.txt` through a real mount
/// marks `dir` opaque with no per-file whiteouts; deletions of snapshot
/// files beneath it must surface in a child's diff (the snapshot is not the
/// child's first base layer), not vanish
#[tokio::test]
async fn opaque_dir_deletions_surface_in_child_diff() {
    if !fuse_available() {
        eprintln!("skipping: fuse-overlayfs unavailable");
        return;
    }
    let rig = harness();
    let (project, commit) = (&rig.project, rig.commit.clone());
    let parent = AgentId::from("op".to_string());
    let child = AgentId::from("kid".to_string());
    project.new_agent_workdir(&commit, &parent).await.unwrap();
    project.mount_agent(&commit, &parent).await.unwrap();

    // the parent leaves `dir` alone, so it lives only in the snapshot —
    // beneath the child's inherited (empty) lower
    project
        .duplicate_agent_workdir(&parent, &child, &commit)
        .await
        .unwrap();
    let child_base = project
        .mint_spawn_base(&child, &commit, &commit)
        .await
        .unwrap();
    project.mount_agent(&commit, &child).await.unwrap();
    let wd = project.agent_workdir(&child);
    fs::remove_dir_all(wd.join("dir")).unwrap();
    fs::create_dir(wd.join("dir")).unwrap();
    fs::write(wd.join("dir/new.txt"), "fresh\n").unwrap();

    // fuse-overlayfs hides its own opacity/whiteout bookkeeping from the
    // composed view, so no `.wh.` marker can reach the rendered diff
    insta::assert_snapshot!(inspect_diff(&project, &child, &child_base), @"
    diff --git a/dir/a.txt b/dir/a.txt
    deleted file mode 100644
    index 7898192..0000000
    --- a/dir/a.txt
    +++ /dev/null
    @@ -1 +0,0 @@
    -a
    diff --git a/dir/b.txt b/dir/b.txt
    deleted file mode 100644
    index 6178079..0000000
    --- a/dir/b.txt
    +++ /dev/null
    @@ -1 +0,0 @@
    -b
    diff --git a/dir/new.txt b/dir/new.txt
    new file mode 100644
    index 0000000..92d5444
    --- /dev/null
    +++ b/dir/new.txt
    @@ -0,0 +1 @@
    +fresh
    ");
}

/// Durability: unmount/restart remount preserves the delta and the diff it
/// feeds in the upper layer.
#[tokio::test]
async fn remount_preserves_delta_and_diff() {
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
    // a primary's base is its own commit: the diff is cumulative
    insta::assert_snapshot!(inspect_diff(&project, &aid, &commit), @"
    diff --git a/doomed.txt b/doomed.txt
    deleted file mode 100644
    index 2d030d7..0000000
    --- a/doomed.txt
    +++ /dev/null
    @@ -1 +0,0 @@
    -delete me
    diff --git a/work.txt b/work.txt
    new file mode 100644
    index 0000000..81fae44
    --- /dev/null
    +++ b/work.txt
    @@ -0,0 +1 @@
    +progress
    ");
}

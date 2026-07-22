use std::os::unix::fs::MetadataExt;
use std::path::Path;

use anyhow::Result;
use tokio::fs::create_dir_all;
use tokio::fs::hard_link;

use super::Overlay;
use crate::deps;
use crate::project::Paths;

impl Overlay {
    pub async fn add_shared_file(
        &self,
        src: &Path,
        dst: &Path,
    ) -> Result<()> {
        if let Some(parent) = dst.parent() {
            create_dir_all(parent).await?;
        }
        hard_link(src, dst).await?;
        Ok(())
    }

    pub async fn add_shared_dir(
        &self,
        paths: &Paths,
        src: &Path,
        dst: &Path,
    ) -> Result<()> {
        create_dir_all(dst).await?;
        let args = [
            src.to_string_lossy().to_string(),
            dst.to_string_lossy().to_string(),
            "--no-allow-other".to_string(),
            "-r".to_string(),
        ];
        let status = paths.run(deps::BINDFS, args).await?.status;
        anyhow::ensure!(status.success(), "bindfs failed with status: {status}");
        Ok(())
    }

    pub async fn init_shared(
        &self,
        paths: &Paths,
    ) -> Result<()> {
        ensure_ignored(&paths.root, &self.shared_paths)?;
        create_dir_all(self.shared(paths)).await?;
        for path in &self.shared_paths {
            let path = Path::new(path);
            let src = paths.root.join(path);
            if !src.exists() {
                continue;
            }

            let dst = self.shared(paths).join(path);
            if src.is_dir() {
                self.add_shared_dir(paths, &src, &dst).await?;
            } else {
                self.add_shared_file(&src, &dst).await?;
            }
        }
        Ok(())
    }

    pub async fn unmount_shared(
        &self,
        paths: &Paths,
    ) -> Result<()> {
        let shared_root = self.shared(paths);
        for mount in proc_mounts::MountIter::new()? {
            // bindfs reports as "fuse", match the prefix just in case
            if let Ok(mount) = mount
                && mount.fstype.starts_with("fuse")
                && mount.dest.starts_with(&shared_root)
            {
                self.unmount(paths, Path::new(&mount.dest)).await?;
            }
        }
        if shared_root.exists() {
            tokio::task::spawn_blocking(move || remove_shared_tree(&shared_root)).await??;
        }
        Ok(())
    }
}

/// check that shared paths are gitignored
fn ensure_ignored(
    root: &Path,
    shared: &[String],
) -> Result<()> {
    let repo = git2::Repository::open(root)?;
    for entry in shared {
        let src = root.join(entry);
        if !src.exists() {
            continue;
        }
        anyhow::ensure!(
            repo.is_path_ignored(entry)?,
            "shared path '{entry}' must be gitignored"
        );
    }
    Ok(())
}

/// recursively delete root without crossing mount points, to avoid deleting the source of a bindfs mount
fn remove_shared_tree(root: &Path) -> Result<()> {
    fn recurse(
        path: &Path,
        root_dev: u64,
    ) -> Result<()> {
        let meta = std::fs::symlink_metadata(path)?;
        if !meta.is_dir() {
            std::fs::remove_file(path)?;
            return Ok(());
        }
        anyhow::ensure!(
            meta.dev() == root_dev,
            "{} is still mounted; refusing to delete through it",
            path.display()
        );
        for entry in std::fs::read_dir(path)? {
            recurse(&entry?.path(), root_dev)?;
        }
        std::fs::remove_dir(path)?;
        Ok(())
    }

    let root_dev = std::fs::symlink_metadata(root)?.dev();
    recurse(root, root_dev)
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;

    fn tmp() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("vicode-shared-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// ignored paths (including dir-only `build/` rules and missing entries)
    /// pass; a non-ignored path fails init before anything mounts
    #[test]
    fn shared_paths_must_be_gitignored() {
        let root = tmp();
        git2::Repository::init(&root).unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n.env\n").unwrap();
        std::fs::create_dir(root.join("build")).unwrap();
        std::fs::write(root.join(".env"), "k=v").unwrap();
        std::fs::write(root.join("src.rs"), "code").unwrap();

        ensure_ignored(&root, &["build".into(), ".env".into(), "missing".into()]).unwrap();
        insta::assert_snapshot!(
            ensure_ignored(&root, &["src.rs".into()]).unwrap_err(),
            @"shared path 'src.rs' must be gitignored"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn removes_hard_link_but_preserves_source() {
        let dir = tmp();
        let src = dir.join("real.txt");
        std::fs::write(&src, b"payload").unwrap();
        let shared = dir.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::hard_link(&src, shared.join("real.txt")).unwrap();

        remove_shared_tree(&shared).unwrap();

        assert!(!shared.exists());
        assert_eq!(std::fs::read(&src).unwrap(), b"payload".to_vec());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn removes_nested_dirs_and_symlinks() {
        let dir = tmp();
        let shared = dir.join("shared");
        std::fs::create_dir_all(shared.join("a/b")).unwrap();
        std::os::unix::fs::symlink("dangling-target", shared.join("link")).unwrap();

        remove_shared_tree(&shared).unwrap();

        assert!(!shared.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

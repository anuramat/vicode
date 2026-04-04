use std::path::Path;

use anyhow::Result;
use tokio::fs::create_dir_all;
use tokio::fs::hard_link;

use super::Overlay;
use crate::deps;
use crate::project::Layout;

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
        layout: &Layout,
        src: &Path,
        dst: &Path,
    ) -> Result<()> {
        create_dir_all(dst).await?;
        let args = [
            src.to_string_lossy().to_string(),
            dst.to_string_lossy().to_string(),
            "--no-allow-other".to_string(),
        ];
        let status = layout.bash(deps::BINDFS, args).await?.status;
        anyhow::ensure!(status.success(), "bindfs failed with status: {status}");
        Ok(())
    }

    pub async fn init_shared(
        &self,
        layout: &Layout,
        shared: &[String],
    ) -> Result<()> {
        create_dir_all(self.shared(layout)).await?;
        for path in shared {
            let path = Path::new(path);
            let src = layout.root.join(path);
            if !src.exists() {
                continue;
            }

            let dst = self.shared(layout).join(path);
            if src.is_dir() {
                self.add_shared_dir(layout, &src, &dst).await?;
            } else {
                self.add_shared_file(&src, &dst).await?;
            }
        }
        Ok(())
    }

    pub async fn unmount_shared(
        &self,
        layout: &Layout,
    ) -> Result<()> {
        let shared_root = self.shared(layout);
        for mount in proc_mounts::MountIter::new()? {
            if let Ok(mount) = mount
                && mount.fstype == "fuse"
                && mount.dest.starts_with(&shared_root)
            {
                self.unmount(layout, Path::new(&mount.dest)).await?;
            }
        }
        if shared_root.exists() {
            tokio::fs::remove_dir_all(shared_root).await?;
        }
        Ok(())
    }
}

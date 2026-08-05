//! whiteout-preserving layer copy (H3): an overlay delta carries deletions
//! as char-device `0:0` whiteouts and opacity as xattr markers — a copy
//! that drops either silently resurrects the parent's deleted files in the
//! child. Replaces the old `fs_extra` copy, which preserved neither.

use std::fs;
use std::path::Path;

use anyhow::Result;

/// recursively copy one delta layer, preserving whiteouts (mknod), xattr
/// markers, and symlinks; skips the top-level `.git` — an agent's own
/// worktree pointer is never inherited
pub fn copy_layer(
    src: &Path,
    dst: &Path,
) -> Result<()> {
    copy_dir(src, dst, true)
}

fn copy_dir(
    src: &Path,
    dst: &Path,
    top: bool,
) -> Result<()> {
    fs::create_dir_all(dst)?;
    copy_xattrs(src, dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        if top && entry.file_name() == ".git" {
            continue;
        }
        let meta = entry.metadata()?;
        let (s, d) = (entry.path(), dst.join(entry.file_name()));
        let ty = meta.file_type();
        if ty.is_dir() {
            copy_dir(&s, &d, false)?;
        } else if ty.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&s)?, &d)?;
        } else if std::os::unix::fs::FileTypeExt::is_char_device(&ty) {
            mknod(&d, &meta)?;
        } else {
            fs::copy(&s, &d)?;
            copy_xattrs(&s, &d)?;
        }
    }
    // restore the directory's own mode (0700, setgid, sticky) last, so a
    // restrictive src dir can't block writing its children first
    fs::set_permissions(dst, fs::symlink_metadata(src)?.permissions())?;
    Ok(())
}

fn mknod(
    path: &Path,
    meta: &fs::Metadata,
) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    #[allow(clippy::cast_possible_truncation)]
    let mode = libc::S_IFCHR | (meta.mode() as libc::mode_t & 0o7777);
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let ret = unsafe { libc::mknod(c.as_ptr(), mode, meta.rdev() as libc::dev_t) };
    anyhow::ensure!(
        ret == 0,
        "whiteout copy mknod({path:?}) failed: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

fn copy_xattrs(
    src: &Path,
    dst: &Path,
) -> Result<()> {
    for attr in xattr::list(src)? {
        if let Some(value) = xattr::get(src, &attr)? {
            xattr::set(dst, &attr, &value)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;

    #[test]
    fn copy_layer_preserves_structure_symlinks_and_xattrs() {
        let root = std::env::temp_dir().join(format!("vicode-copy-{}", uuid::Uuid::new_v4()));
        let src = root.join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("file"), "content").unwrap();
        fs::write(src.join("sub/nested"), "nested").unwrap();
        std::os::unix::fs::symlink("file", src.join("link")).unwrap();
        fs::create_dir_all(src.join(".git")).unwrap();
        fs::write(src.join(".git/config"), "junk").unwrap();
        // xattr marker (opaque-dir analogue); tmpfs without user-xattr
        // support skips the assertion
        let marked = xattr::set(src.join("sub"), "user.overlay.test", b"y").is_ok();

        let dst = root.join("dst");
        copy_layer(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("file")).unwrap(), "content");
        assert_eq!(
            fs::read_to_string(dst.join("sub/nested")).unwrap(),
            "nested"
        );
        assert_eq!(
            fs::read_link(dst.join("link")).unwrap(),
            std::path::PathBuf::from("file")
        );
        assert!(!dst.join(".git").exists());
        if marked {
            assert_eq!(
                xattr::get(dst.join("sub"), "user.overlay.test").unwrap(),
                Some(b"y".to_vec())
            );
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn copy_layer_preserves_directory_mode() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("vicode-mode-{}", uuid::Uuid::new_v4()));
        let src = root.join("src");
        fs::create_dir_all(src.join("dir")).unwrap();
        fs::write(src.join("dir/f"), "x").unwrap();
        // setgid + rwxr-x---: mode is set last, so the restrictive dir still
        // takes its children first
        fs::set_permissions(src.join("dir"), fs::Permissions::from_mode(0o2750)).unwrap();

        let dst = root.join("dst");
        copy_layer(&src, &dst).unwrap();

        assert_eq!(
            fs::metadata(dst.join("dir")).unwrap().permissions().mode() & 0o7777,
            0o2750
        );
        fs::remove_dir_all(root).ok();
    }
}

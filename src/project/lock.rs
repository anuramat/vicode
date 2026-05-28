use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::sync::Arc;

use anyhow::Result;
use fs4::FileExt;
use fs4::TryLockError;

use crate::project::layout::LayoutTrait;

#[must_use = "should be kept throughout the lifetime of the app"]
#[derive(Clone, Debug)]
pub struct ProjectLock {
    _file: Arc<File>,
}

impl ProjectLock {
    pub fn acquire(layout: &impl LayoutTrait) -> Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(layout.project_lock())?;
        let project_id = layout.id();
        match FileExt::try_lock(&file) {
            Ok(()) => {
                write_pid(&mut file)?;
                Ok(Self {
                    _file: Arc::new(file),
                })
            }
            Err(TryLockError::WouldBlock) => {
                let pid = read_pid(&mut file);
                anyhow::bail!("vicode is already running in {project_id} (PID: {pid})");
            }
            Err(TryLockError::Error(err)) => {
                let err: anyhow::Error = err.into();
                Err(err.context("failed to acquire project lock for {project}"))
            }
        }
    }
}

fn write_pid(file: &mut File) -> Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    writeln!(file, "{}", std::process::id())?;
    file.sync_data()?;
    Ok(())
}

fn read_pid(file: &mut File) -> String {
    let mut pid = String::new();
    if file.seek(SeekFrom::Start(0)).is_ok() {
        drop(file.read_to_string(&mut pid));
    }
    let pid = pid.trim();
    if pid.is_empty() {
        return "unknown".into();
    }
    pid.into()
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::project::Layout;

    #[test]
    fn second_lock_reports_project_and_pid() {
        let root = std::env::temp_dir().join(format!("vicode-lock-{}", uuid::Uuid::new_v4()));
        let data = root.join(".vicode");
        std::fs::create_dir_all(&data).unwrap();
        let layout = Layout {
            root,
            id: "test-project".into(),
            data,
        };

        let _lock = ProjectLock::acquire(&layout).unwrap();
        let err = ProjectLock::acquire(&layout).unwrap_err();
        let msg = err.to_string();

        assert_eq!(
            msg,
            format!(
                "vicode is already running in test-project (PID: {})",
                std::process::id(),
            )
        );
    }
}

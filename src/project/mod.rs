pub mod agent;
pub mod app;
pub mod git;
pub mod instructions;
pub mod layout;
pub mod overlay;

use std::path::PathBuf;

use anyhow::Result;
use anyhow::anyhow;
use git2::Repository;
use tokio::process::Command;

use crate::config::DIRS;

lazy_static::lazy_static! {
    pub static ref PROJECT: Project = Project::new().unwrap();
}

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    /// path-based unique identifier for the project
    pub id: String,
    /// per-project data directory
    pub data: PathBuf,
}

impl Project {
    fn id(repo_root: PathBuf) -> String {
        let name_prefix = if let Some(name) = repo_root.file_name() {
            format!("{}_", name.to_string_lossy())
        } else {
            String::new()
        };
        let uuid = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            repo_root.to_string_lossy().as_bytes(),
        )
        .to_string();
        format!("{}{}", name_prefix, uuid)
    }

    pub fn new() -> Result<Self> {
        // TODO discover vs open? normalize across codebase
        let repo = Repository::discover(".")?;
        let root = repo
            .workdir()
            .ok_or(anyhow!("cannot run inside a bare repository"))?
            .to_path_buf();
        let id = Self::id(root.clone());
        let data = DIRS.create_data_directory(&id)?;
        let project = Self { root, id, data };
        std::fs::create_dir_all(project.snapshots())?;
        Ok(project)
    }

    /// run bash command in the root directory
    pub async fn bash<I, S>(
        &self,
        command: &str,
        args: I,
    ) -> Result<std::process::Output>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let output = Command::new(command)
            .current_dir(self.root.clone())
            .args(args.into_iter().map(Into::into))
            .output()
            .await?;
        Ok(output)
    }
}

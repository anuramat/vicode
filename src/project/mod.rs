pub mod backend;
pub mod layout;

use std::path::PathBuf;

use ambassador::Delegate;
use anyhow::Result;
use anyhow::anyhow;
use git2::Repository;

use crate::agent::*;
use crate::config::Config;
use crate::config::DIRS;
use crate::config::INSTRUCTIONS;
use crate::project::backend::Backend;
use crate::project::backend::BackendKind;
use crate::project::backend::Copy;
use crate::project::backend::Overlay;
use crate::project::layout::*;

#[derive(Clone, Delegate)]
#[delegate(LayoutTrait, target = "layout")]
pub struct Project {
    layout: Layout,
    backend: BackendKind,
    config: Config,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub root: PathBuf,
    /// path-based unique identifier for the project
    pub id: String,
    /// per-project data directory
    pub data: PathBuf,
}

impl Project {
    fn backend(config: &Config) -> BackendKind {
        if config.disable_overlay {
            BackendKind::Copy(Copy)
        } else {
            BackendKind::Overlay(Overlay)
        }
    }

    pub fn name(&self) -> String {
        self.layout
            .root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    }

    fn id(root: PathBuf) -> String {
        let name_prefix = if let Some(name) = root.file_name() {
            format!("{}_", name.to_string_lossy())
        } else {
            String::new()
        };
        let uuid = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            root.to_string_lossy().as_bytes(),
        )
        .to_string();
        format!("{}{}", name_prefix, uuid)
    }

    pub fn new(config: Config) -> Result<Self> {
        // TODO discover vs open? normalize across codebase
        let repo = Repository::discover(".")?;
        let root = repo
            .workdir()
            .ok_or(anyhow!("cannot run inside a bare repository"))?
            .to_path_buf();
        let id = Self::id(root.clone());
        let data = DIRS.create_data_directory(&id)?;
        Ok(Self {
            layout: Layout { root, id, data },
            backend: Self::backend(&config),
            config,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    #[cfg(test)]
    pub fn new_test() -> Result<Self> {
        let config = Config::test();
        let root = std::env::temp_dir().join(format!("vicode-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let repo = Repository::init(&root)?;
        let tree_id = repo.index()?.write_tree()?;
        let tree = repo.find_tree(tree_id)?;
        let signature = git2::Signature::now("vicode", "vicode@example.com")?;
        repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])?;
        let data = root.join(".vicode");
        std::fs::create_dir_all(&data)?;
        Ok(Self {
            layout: Layout {
                id: Self::id(root.clone()),
                root,
                data,
            },
            backend: Self::backend(&config),
            config,
        })
    }

    pub async fn mount_agent(
        &self,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()> {
        self.backend.mount_agent(&self.layout, commit, aid).await
    }

    pub async fn init(&self) -> Result<()> {
        self.backend.init(&self.layout, self.config()).await
    }

    pub async fn unmount_all(&self) -> Result<()> {
        self.backend.unmount_all(&self.layout).await
    }

    pub fn agent_changes_dir(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.backend.agent_changes_dir(&self.layout, aid)
    }

    pub async fn instructions(
        &self,
        aid: &AgentId,
    ) -> Result<String> {
        use std::io::ErrorKind;
        let mut collected = INSTRUCTIONS.clone();
        let root = self.agent(aid);
        for name in &self.config.context_files {
            match tokio::fs::read_to_string(root.join(name)).await {
                Ok(text) => collected.push_str(&text),
                Err(err) if err.kind() == ErrorKind::NotFound => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Ok(collected)
    }

    pub async fn duplicate_agent(
        &self,
        src_id: &AgentId,
        aid: &AgentId,
        state: &AgentState,
        git: bool,
    ) -> Result<()> {
        self.backend
            .duplicate_agent(&self.layout, src_id, aid, state, git)
            .await
    }

    pub async fn unmount_agent(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        self.backend.unmount_agent(&self.layout, aid).await
    }

    pub async fn new_agent(
        &self,
        commit: &str,
        aid: &AgentId,
        git: bool,
    ) -> Result<()> {
        self.backend.new_agent(&self.layout, commit, aid, git).await
    }
}

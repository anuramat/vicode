pub mod layout;
pub mod overlay;

use std::path::PathBuf;
use std::sync::LazyLock;

use ambassador::Delegate;
use ambassador::delegatable_trait;
use anyhow::Result;
use anyhow::anyhow;
use git2::Repository;

use crate::agent::*;
use crate::config::CONFIG;
use crate::config::DIRS;
use crate::config::INSTRUCTIONS;
use crate::project::layout::*;

pub static PROJECT: LazyLock<Project> = LazyLock::new(|| Project::new().unwrap());

#[derive(Delegate)]
#[delegate(LayoutTrait, target = "layout")]
pub struct Project {
    layout: Layout,
    pub backend: BackendKind,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub root: PathBuf,
    /// path-based unique identifier for the project
    pub id: String,
    /// per-project data directory
    pub data: PathBuf,
}

#[derive(Debug, Clone, Delegate)]
#[delegate(Backend)]
pub enum BackendKind {
    Overlay(Overlay),
}

#[derive(Debug, Clone)]
pub struct Overlay;

#[async_trait::async_trait]
#[delegatable_trait]
pub trait Backend {
    fn agent_changes_dir(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf;
    async fn init(
        &self,
        layout: &Layout,
    ) -> Result<()>;
    async fn new_agent(
        &self,
        layout: &Layout,
        commit: &str,
        aid: &AgentId,
        git: bool,
    ) -> Result<()>;
    async fn mount_agent(
        &self,
        layout: &Layout,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()>;
    async fn unmount_agent(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> Result<()>;
    async fn unmount_all(
        &self,
        layout: &Layout,
    ) -> Result<()>;
    async fn duplicate_agent(
        &self,
        layout: &Layout,
        src_id: &AgentId,
        aid: &AgentId,
        state: &AgentState,
        git: bool,
    ) -> Result<()>;
    async fn add_worktree(
        &self,
        layout: &Layout,
        aid: &AgentId,
        commit: &str,
        name: &str,
    ) -> Result<()>;
}

impl Project {
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

    pub fn new() -> Result<Self> {
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
            backend: BackendKind::Overlay(Overlay),
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
        self.backend.init(&self.layout).await
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
        for name in &CONFIG.context_files {
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

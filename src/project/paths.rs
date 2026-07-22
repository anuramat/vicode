use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use derive_getters::Getters;
use git2::Repository;

use crate::agent::id::AgentId;
use crate::config::DIRS;

const AGENTS_DIRNAME: &str = "agents";
pub const AGENT_WORKDIR_DIRNAME: &str = "workdir";
const PROJECT_LOCK_FILENAME: &str = "project.lock";
const STATE_FILENAME: &str = "state.redb";
const WORKTREE_NAME_PREFIX: &str = "vc-";
const BASE_REF_PREFIX: &str = "refs/vicode/base/";

#[derive(Debug, Clone, Getters)]
pub struct Paths {
    /// root of the git repository
    pub root: PathBuf,
    /// path-based unique identifier for the project
    pub id: String,
    /// per-project data directory
    pub data: PathBuf,
}

pub fn worktree_name_to_agent_id(name: &str) -> Option<AgentId> {
    name.strip_prefix(WORKTREE_NAME_PREFIX)
        .map(|s| AgentId::from(s.to_string()))
}

pub fn base_ref_to_agent_id(name: &str) -> Option<AgentId> {
    name.strip_prefix(BASE_REF_PREFIX)
        .map(|s| AgentId::from(s.to_string()))
}

impl Paths {
    /// discover the project paths from the current working directory
    pub fn new() -> Result<Self> {
        // TODO discover vs open? normalize across codebase
        let repo = Repository::discover(".")?;
        let root = repo
            .workdir()
            .context("cannot run inside a bare repository")?
            .to_path_buf();
        let id = Self::derive_id(&root);
        let data = DIRS.create_data_directory(&id)?;
        Ok(Self { root, id, data })
    }

    pub fn derive_id(root: &Path) -> String {
        let name_prefix = root
            .file_name()
            .map(|name| format!("{}_", name.to_string_lossy()))
            .unwrap_or_default();
        let uuid = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            root.to_string_lossy().as_bytes(),
        )
        .to_string();
        format!("{name_prefix}{uuid}")
    }

    pub fn gitdir(&self) -> Result<PathBuf> {
        let repo = Repository::open(self.root())?;
        Ok(repo.commondir().to_path_buf())
    }

    pub fn state_db(&self) -> PathBuf {
        self.data().join(STATE_FILENAME)
    }

    pub fn project_lock(&self) -> PathBuf {
        self.data().join(PROJECT_LOCK_FILENAME)
    }

    pub fn agents(&self) -> PathBuf {
        self.data().join(AGENTS_DIRNAME)
    }

    pub fn agent(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.agents().join(aid.to_string())
    }

    pub fn agent_workdir(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.agent(aid).join(AGENT_WORKDIR_DIRNAME)
    }

    pub fn worktree_name(
        &self,
        aid: &AgentId,
    ) -> String {
        format!("{WORKTREE_NAME_PREFIX}{aid}")
    }

    pub fn base_ref(
        &self,
        aid: &AgentId,
    ) -> String {
        format!("{BASE_REF_PREFIX}{aid}")
    }
}

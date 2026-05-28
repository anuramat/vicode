use std::path::Path;
use std::path::PathBuf;

use ambassador::delegatable_trait;
use anyhow::Result;
use git2::Repository;

use super::Layout;
use crate::agent::id::AgentId;

const AGENTS_DIRNAME: &str = "agents";
pub const AGENT_WORKDIR_DIRNAME: &str = "workdir";
const PROJECT_LOCK_FILENAME: &str = "project.lock";
const STATE_DB_FILENAME: &str = "state.redb";
const WORKTREE_NAME_PREFIX: &str = "vc-";

#[delegatable_trait]
pub trait LayoutTrait {
    fn root(&self) -> &std::path::Path;

    fn data(&self) -> &std::path::Path;

    fn id(&self) -> &str;

    fn gitdir(&self) -> Result<PathBuf> {
        let repo = Repository::open(self.root())?;
        Ok(repo.commondir().to_path_buf())
    }

    fn state_db(&self) -> PathBuf {
        self.data().join(STATE_DB_FILENAME)
    }

    fn project_lock(&self) -> PathBuf {
        self.data().join(PROJECT_LOCK_FILENAME)
    }

    fn agents(&self) -> PathBuf {
        self.data().join(AGENTS_DIRNAME)
    }

    fn agent(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.agents().join(aid.to_string())
    }

    fn agent_workdir(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.agent(aid).join(AGENT_WORKDIR_DIRNAME)
    }

    fn worktree_name(
        &self,
        aid: &AgentId,
    ) -> String {
        format!("{WORKTREE_NAME_PREFIX}{aid}")
    }
}

pub fn worktree_name_to_agent_id(name: &str) -> Option<AgentId> {
    name.strip_prefix(WORKTREE_NAME_PREFIX)
        .map(|s| AgentId::from(s.to_string()))
}

impl LayoutTrait for Layout {
    fn root(&self) -> &Path {
        &self.root
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn data(&self) -> &Path {
        &self.data
    }
}

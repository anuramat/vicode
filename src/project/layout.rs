use std::path::PathBuf;

use ambassador::delegatable_trait;
use anyhow::Result;
use git2::Repository;

use super::Layout;
use crate::agent::id::AgentId;

const APP_STATE_FILENAME: &str = "state.json";
const AGENTS_DIRNAME: &str = "agents";
const AGENT_STATE_FILENAME: &str = "state.json";
pub const AGENT_WORKDIR_DIRNAME: &str = "workdir";
const WORKTREE_NAME_PREFIX: &str = "vc-";

#[async_trait::async_trait]
#[delegatable_trait]
pub trait LayoutTrait {
    fn root(&self) -> PathBuf;

    fn data(&self) -> PathBuf;

    fn id(&self) -> String;

    fn app_state(&self) -> PathBuf {
        self.data().join(APP_STATE_FILENAME)
    }

    fn gitdir(&self) -> Result<PathBuf> {
        let repo = Repository::open(self.root())?;
        Ok(repo.commondir().to_path_buf())
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

    fn agent_state(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.agent(aid).join(AGENT_STATE_FILENAME)
    }

    fn worktree_name(
        &self,
        aid: &AgentId,
    ) -> String {
        format!("{}{}", WORKTREE_NAME_PREFIX, aid)
    }

    async fn agent_id_exists(
        &self,
        aid: &AgentId,
    ) -> Result<bool> {
        Ok(tokio::fs::try_exists(self.agent(aid)).await?)
    }
}

#[async_trait::async_trait]
impl LayoutTrait for Layout {
    fn root(&self) -> PathBuf {
        self.root.clone()
    }

    fn id(&self) -> String {
        self.id.clone()
    }

    fn data(&self) -> PathBuf {
        self.data.clone()
    }
}

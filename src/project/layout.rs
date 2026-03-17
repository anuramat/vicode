use std::path::PathBuf;

use anyhow::Result;
use git2::Repository;

use super::Project;
use crate::agent::id::AgentId;

const APP_STATE_FILENAME: &str = "state.json";
const SNAPSHOTS_DIRNAME: &str = "snapshots";
const AGENTS_DIRNAME: &str = "agents";

const AGENT_STATE_FILENAME: &str = "state.json";
const AGENT_WORKDIR_DIRNAME: &str = "workdir";
const OVERLAY_DIRNAME: &str = ".overlay";

const OVERLAY_UPPER_DIRNAME: &str = "upper";
const OVERLAY_WORKDIR_DIRNAME: &str = "workdir";

const WORKTREE_NAME_PREFIX: &str = "vc-worktree-";

impl Project {
    pub fn app_state(&self) -> PathBuf {
        self.data.join(APP_STATE_FILENAME)
    }

    pub fn gitdir(&self) -> Result<PathBuf> {
        let repo = Repository::open(self.root.clone())?;
        Ok(repo.commondir().to_path_buf())
    }

    pub fn worktree_name(aid: &AgentId) -> String {
        format!("{}{}", WORKTREE_NAME_PREFIX, aid.0)
    }

    pub fn overlay(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.agent(aid).join(OVERLAY_DIRNAME)
    }

    pub fn overlay_workdir(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.overlay(aid).join(OVERLAY_WORKDIR_DIRNAME)
    }

    pub fn overlay_upper(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.overlay(aid).join(OVERLAY_UPPER_DIRNAME)
    }

    pub fn agents(&self) -> PathBuf {
        self.data.join(AGENTS_DIRNAME)
    }

    pub fn agent(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        let name = aid.0.to_string();
        self.agents().join(name)
    }

    pub fn agent_workdir(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.agent(aid).join(AGENT_WORKDIR_DIRNAME)
    }

    pub fn agent_state(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.agent(aid).join(AGENT_STATE_FILENAME)
    }

    pub fn snapshots(&self) -> PathBuf {
        self.data.join(SNAPSHOTS_DIRNAME)
    }

    pub fn snapshot(
        &self,
        commit: &str,
    ) -> PathBuf {
        self.snapshots().join(commit)
    }
}

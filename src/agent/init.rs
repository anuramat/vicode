use std::sync::Arc;

use anyhow::Result;
use fs_extra::dir::copy;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;

use crate::agent::task::AgentTaskManager;
use crate::agent::*;
use crate::llm::api::assistant::Assistant;
use crate::llm::history::History;
use crate::project::PROJECT;

const CHANNEL_CAPACITY: usize = 100;

impl Agent {
    /// create new agent from scratch
    pub async fn new(
        parent_tx: Sender<ParentEvent>,
        id: AgentId,
        commit: String,
        instructions: String,
    ) -> Result<Self> {
        let state = AgentState::new(id.clone(), commit, instructions).await?;
        Self::from_state(parent_tx, id, state).await
    }

    /// load agent by id from disk
    pub async fn load(
        parent_tx: Sender<ParentEvent>,
        id: AgentId,
    ) -> Result<Self> {
        let state = PROJECT.load_agent_state(&id).await?;
        Self::from_state(parent_tx, id, state).await
    }

    /// shared logic
    async fn from_state(
        parent: Sender<ParentEvent>,
        id: AgentId,
        state: AgentState,
    ) -> Result<Self> {
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        Ok(Self {
            id,
            state,
            parent,
            rx,
            tskmgr: AgentTaskManager::new(),
            tx,
            assistant: Arc::new(Assistant::new().await?),
            tools: Default::default(),
        })
    }

    pub async fn save(&self) -> Result<()> {
        self.state.save(&self.id).await
    }

    /// clone agent to given id on manual request from UI
    pub async fn try_duplicate(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        anyhow::ensure!(
            self.tskmgr.pending.is_empty(),
            "cannot duplicate while tasks are running"
        );
        duplicate(&self.id, &aid, &self.state, true).await?;
        self.parent.send(ParentEvent::AttachAgent(aid)).await?;
        Ok(())
    }
}

pub async fn duplicate(
    src_id: &AgentId,
    aid: &AgentId,
    state: &AgentState,
    git: bool,
) -> Result<()> {
    {
        let src = PROJECT.overlay_upper(src_id);
        let dst = PROJECT.overlay_upper(aid);
        let opts = fs_extra::dir::CopyOptions::new().copy_inside(true);
        copy(src, dst, &opts)?;
        tokio::fs::remove_file(PROJECT.overlay_upper(aid).join(".git")).await?;
    }
    PROJECT
        .init_overlay(&state.context.commit, aid, git)
        .await?;
    state.save(aid).await
}

impl AgentState {
    /// init a primary agent from scratch
    async fn new(
        id: AgentId,
        commit: String,
        instructions: String,
    ) -> Result<Self> {
        PROJECT.init_overlay(&commit, &id, true).await?;
        let state = Self {
            topology: AgentTopology {
                children: Vec::new(),
                kind: AgentKind::Primary,
            },
            context: AgentContext {
                commit,
                history: History::new(),
                instructions,
            },
        };
        state.save(&id).await?;
        Ok(state)
    }

    pub async fn save(
        &self,
        id: &AgentId,
    ) -> Result<()> {
        PROJECT.save_agent_state(id, self).await
    }
}

use anyhow::Result;
use futures::future::Abortable;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;

use crate::agent::handle::ParentEvent;
use crate::agent::handle::ParentHandle;
use crate::agent::handle::ParentSink;
use crate::agent::task::manager::AgentTaskManager;
use crate::agent::*;
use crate::llm::history::History;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::project::PROJECT;
use crate::project::layout::LayoutTrait;

const CHANNEL_CAPACITY: usize = 100;

// TODO this should store `aid` just like AppEvent implementation
struct ChannelParentSink(Sender<ParentEvent>);

#[async_trait::async_trait]
impl ParentSink for ChannelParentSink {
    async fn send(
        &self,
        event: ParentEvent,
    ) -> Result<()> {
        self.0
            .send(event)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    fn sibling(
        &self,
        _aid: AgentId,
    ) -> ParentHandle {
        Box::new(Self(self.0.clone()))
    }
}

// XXX after refactoring task manager, this should instead translate parent events into AgentEvent::* or something
pub fn channel_parent_sink(tx: Sender<ParentEvent>) -> ParentHandle {
    Box::new(ChannelParentSink(tx))
}

impl Agent {
    /// create new agent from scratch
    pub async fn new(
        parent: ParentHandle,
        id: AgentId,
        commit: String,
        instructions: String,
    ) -> Result<Self> {
        let state = AgentState::new(id.clone(), commit, instructions).await?;
        Self::from_state(parent, id, state).await
    }

    /// load agent by id from disk
    pub async fn load(
        parent: ParentHandle,
        id: AgentId,
    ) -> Result<Self> {
        let path = PROJECT.agent_state(&id);
        let serialized = tokio::fs::read_to_string(path).await?;
        let mut state: AgentState = serde_json::from_str(&serialized)?;
        state.context.history.count_tokens();

        Self::from_state(parent, id, state).await
    }

    /// shared logic
    async fn from_state(
        parent: ParentHandle,
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
            tools: Default::default(),
        })
    }

    pub fn spawn(self) {
        // XXX make sure this works fine
        let (abort, reg) = futures::future::AbortHandle::new_pair();
        tokio::spawn(async move {
            let _ = Abortable::new(self.run(abort), reg).await;
        });
    }

    pub async fn save(&self) -> Result<()> {
        self.state.save(&self.id).await
    }

    /// clone agent to given id on manual request from UI
    pub async fn try_duplicate(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        PROJECT
            .duplicate_agent(&self.id, &aid, &self.state, true)
            .await?;
        Agent::load(self.parent.sibling(aid.clone()), aid)
            .await?
            .spawn();
        Ok(())
    }

    pub async fn delete_agent(&self) -> Result<()> {
        let aid = &self.id;
        PROJECT.unmount_agent(aid).await?;
        Ok(tokio::fs::remove_dir_all(PROJECT.agent(aid)).await?)
    }
}

impl AgentState {
    /// init a primary agent from scratch
    async fn new(
        id: AgentId,
        commit: String,
        instructions: String,
    ) -> Result<Self> {
        PROJECT.new_agent(&commit, &id, true).await?;
        let state = Self {
            status: AgentStatus::Idle,
            assistant: ASSISTANT_POOL
                .get()
                .unwrap()
                .assistant(&ASSISTANT_POOL.get().unwrap().next_primary())?,
            topology: AgentTopology {
                children: Vec::new(),
                kind: AgentKind::Primary,
            },
            context: AgentContext {
                commit,
                history: History::with_instructions(instructions),
            },
        };
        state.save(&id).await?;
        Ok(state)
    }

    pub async fn save(
        &self,
        id: &AgentId,
    ) -> Result<()> {
        let serialized = serde_json::to_string_pretty(self)?;
        let path = PROJECT.agent_state(id);
        tokio::fs::write(path, serialized).await?;
        Ok(())
    }
}

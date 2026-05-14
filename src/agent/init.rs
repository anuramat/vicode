use anyhow::Context;
use anyhow::Result;
use futures::future::Abortable;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;

use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::AgentKind;
use crate::agent::AgentState;
use crate::agent::AgentStatus;
use crate::agent::AgentTopology;
use crate::agent::ToolSchemas;
use crate::agent::handle::ParentEvent;
use crate::agent::handle::ParentHandle;
use crate::agent::handle::ParentSink;
use crate::agent::task::manager::AgentTaskManager;
use crate::llm::history::History;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::project::Project;
use crate::project::layout::LayoutTrait;

const CHANNEL_CAPACITY: usize = 100;

#[derive(Debug)]
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

pub fn channel_parent_sink(tx: Sender<ParentEvent>) -> ParentHandle {
    Box::new(ChannelParentSink(tx))
}

impl Agent {
    /// create new agent from scratch
    pub async fn new(
        project: Project,
        parent: ParentHandle,
        id: AgentId,
        commit: String,
        instructions: String,
    ) -> Result<Self> {
        project.new_agent_workdir(&commit, &id, true).await?;
        let state = AgentState::new(commit, instructions)?;
        state.save(&project, &id).await?;
        Ok(Self::from_state(project, parent, id, state))
    }

    /// load agent by id from disk
    pub async fn load(
        project: Project,
        parent: ParentHandle,
        id: AgentId,
    ) -> Result<Self> {
        let path = project.agent_state(&id);
        let serialized = tokio::fs::read_to_string(path).await?;
        let state: AgentState = serde_json::from_str(&serialized)?;

        Ok(Self::from_state(project, parent, id, state))
    }

    /// shared logic
    fn from_state(
        project: Project,
        parent: ParentHandle,
        id: AgentId,
        state: AgentState,
    ) -> Self {
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        let mut tools = ToolSchemas::default();
        if matches!(state.topology.kind, AgentKind::Subagent { .. }) {
            // TODO use a const from subagent module
            // TODO allow recursive calls with max depth from config
            tools = tools
                .iter()
                .filter(|tool| tool.name != "subagent")
                .cloned()
                .collect::<Vec<_>>()
                .into();
        }
        Self {
            project,
            id,
            state,
            parent,
            rx,
            tskmgr: AgentTaskManager::new(),
            tx,
            tools,
        }
    }

    pub fn spawn(self) {
        let (abort, reg) = futures::future::AbortHandle::new_pair();
        tokio::spawn(async move {
            let _ = Abortable::new(self.run(abort), reg).await;
        });
    }

    pub async fn save(&self) -> Result<()> {
        self.state.save(&self.project, &self.id).await
    }

    /// clone agent to given id on manual request from UI
    pub async fn try_duplicate(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        self.project
            .duplicate_agent_workdir(&self.id, &aid, &self.state.context.commit, true)
            .await?;
        let agent = Self::from_state(
            self.project.clone(),
            self.parent.sibling(aid.clone()),
            aid,
            self.state.clone(),
        );
        agent.save().await?;
        agent.spawn();
        Ok(())
    }

    pub async fn subagent(
        &self,
        parent: ParentHandle,
        inherit_context: bool,
    ) -> Result<Self> {
        let state = AgentState {
            status: AgentStatus::default(),
            assistant: ASSISTANT_POOL
                .get()
                .context("assistant pool not initialized")?
                .next_subagent(&self.state.assistant.id)?,
            topology: AgentTopology {
                children: Vec::new(),
                kind: AgentKind::Subagent {
                    parent: self.id.clone(),
                },
            },
            context: self.state.context.subagent(inherit_context),
        };
        let id = AgentId::new(&self.project).await?;
        let agent = Self::from_state(self.project.clone(), parent, id, state);
        self.project
            .duplicate_agent_workdir(&self.id, &agent.id, &self.state.context.commit, false)
            .await?;
        agent.save().await?;
        Ok(agent)
    }
}

impl AgentState {
    /// init a primary agent from scratch
    fn new(
        commit: String,
        instructions: String,
    ) -> Result<Self> {
        let state = Self {
            status: AgentStatus::default(),
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
                history: History::new(instructions),
            },
        };
        Ok(state)
    }

    pub async fn save(
        &self,
        layout: &impl LayoutTrait,
        id: &AgentId,
    ) -> Result<()> {
        let serialized = serde_json::to_string_pretty(self)?;
        let path = layout.agent_state(id);
        tokio::fs::write(path, serialized).await?;
        Ok(())
    }
}

use anyhow::Result;
use futures::future::Abortable;
use tokio::sync::mpsc::channel;

use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::AgentStatus;
use crate::agent::AgentVisibility;
use crate::agent::ToolSchemas;
use crate::agent::router::AgentRouterHandle;
use crate::agent::router::RuntimeHandle;
use crate::agent::task::manager::AgentTaskManager;
use crate::llm::history::History;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::project::Project;
use crate::project::layout::LayoutTrait;

const CHANNEL_CAPACITY: usize = 100;

impl Agent {
    /// create new agent from scratch
    pub async fn new(
        project: Project,
        router: AgentRouterHandle,
        id: AgentId,
        commit: String,
        instructions: String,
    ) -> Result<Self> {
        project.new_agent_workdir(&commit, &id, true).await?;
        let state = AgentState::new(commit, instructions)?;
        state.save(&project, &id).await?;
        Ok(Self::from_state(project, router, id, state))
    }

    /// load agent by id from disk
    pub async fn load(
        project: Project,
        router: AgentRouterHandle,
        id: AgentId,
    ) -> Result<Self> {
        let path = project.agent_state(&id);
        let serialized = tokio::fs::read_to_string(path).await?;
        let state: AgentState = serde_json::from_str(&serialized)?;

        Ok(Self::from_state(project, router, id, state))
    }

    /// shared logic
    pub fn from_state(
        project: Project,
        router: AgentRouterHandle,
        id: AgentId,
        state: AgentState,
    ) -> Self {
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        let mut tools = ToolSchemas::default();
        if matches!(state.visibility, AgentVisibility::Hidden) {
            // TODO allow recursive calls with max depth from config
            tools = tools
                .iter()
                .filter(|tool| tool.name != crate::tools::subagent::TOOL_NAME)
                .cloned()
                .collect::<Vec<_>>()
                .into();
        }
        Self {
            project,
            id,
            state,
            router,
            pending_done: None,
            rx,
            tskmgr: AgentTaskManager::new(),
            tx,
            tools,
        }
    }

    /// Spawn the agent's run loop, returning the runtime handle the router
    /// uses to mailbox commands and to abort.
    pub fn spawn(self) -> RuntimeHandle {
        let (abort, reg) = futures::future::AbortHandle::new_pair();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = Abortable::new(self.run(), reg).await;
        });
        RuntimeHandle::new(tx, abort)
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
            self.router.clone(),
            aid.clone(),
            self.state.clone(),
        );
        agent.save().await?;
        let runtime = agent.spawn();
        self.router.register(aid, runtime).await?;
        Ok(())
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
            visibility: AgentVisibility::Tab,
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

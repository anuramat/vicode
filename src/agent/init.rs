use anyhow::Result;
use futures::future::Abortable;
use tokio::sync::mpsc::channel;

use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::AgentStatus;
use crate::agent::router::AgentRouterHandle;
use crate::agent::router::RuntimeHandle;
use crate::agent::task::manager::AgentTaskManager;
use crate::agent::tool::registry::TOOL_REGISTRY;
use crate::agent::tool::registry::ToolRegistry;
use crate::llm::history::History;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::project::Project;

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
        let max_depth = project.config().subagent_max_depth;
        let state = AgentState::new(commit, instructions, max_depth)?;
        state.save(&project, &id).await?;
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
        let tools = tools_for_depth(state.max_depth);
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

/// Tool set for an agent with `max_depth` remaining subagent budget.
fn tools_for_depth(max_depth: u32) -> ToolRegistry {
    if max_depth > 0 {
        return TOOL_REGISTRY.clone();
    }
    TOOL_REGISTRY.without([crate::tools::subagent::TOOL_NAME])
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::agent::router::AgentRouter;
    use crate::config::Config;
    use crate::llm::provider::assistant::AssistantPool;
    use crate::project::layout::LayoutTrait;

    async fn assistant() -> crate::llm::provider::assistant::Assistant {
        let pool = ASSISTANT_POOL
            .get_or_init(|| async {
                AssistantPool::from_config(
                    &Config::parse_with_defaults(
                        r#"
                primary_assistant = ["test"]
                shell_cmd = ["bash", "-c"]

                [sandbox]
                kind = "bwrap"
                bin = "bwrap"
                args = []
                stages = []

                [providers.main]
                api = "responses"
                base_url = "https://api.example.com/v1"

                [assistants.test]
                provider = "main"
                model = "gpt-test"
                window = 1
                "#,
                    )
                    .unwrap(),
                )
                .await
                .unwrap()
            })
            .await;
        pool.assistant(&pool.next_primary()).unwrap()
    }

    #[tokio::test]
    async fn try_duplicate_registers_child_with_router() {
        let project = Project::new_test().unwrap();
        let (app_tx, _app_rx) = channel(8);
        let router = AgentRouter::spawn(app_tx, project.clone(), Default::default());

        let parent_aid = AgentId::from(format!("dup-parent-{}", uuid::Uuid::new_v4()));
        let parent_workdir = project.agent_workdir(&parent_aid);
        tokio::fs::create_dir_all(&parent_workdir).await.unwrap();
        let repo = git2::Repository::open(project.root()).unwrap();
        let commit = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();

        let state = AgentState {
            status: AgentStatus::default(),
            assistant: assistant().await,
            max_depth: 1,
            context: AgentContext {
                commit: commit.clone(),
                history: History::new("".into()),
            },
        };
        let parent = Agent::from_state(project.clone(), router.clone(), parent_aid.clone(), state);

        let child_aid = router.allocate_agent_id().await.unwrap();
        parent.try_duplicate(child_aid.clone()).await.unwrap();

        assert!(!project.agent(&child_aid).join("state.json").exists());

        // observable via router: deletion succeeds only if registered
        router.delete(child_aid).await.unwrap();
    }

    #[tokio::test]
    async fn new_agent_saves_without_state_json() {
        assistant().await;
        let project = Project::new_test().unwrap();
        let (app_tx, _app_rx) = channel(8);
        let router = AgentRouter::spawn(app_tx, project.clone(), Default::default());
        let aid = AgentId::from(format!("new-agent-{}", uuid::Uuid::new_v4()));
        let repo = git2::Repository::open(project.root()).unwrap();
        let commit = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();

        Agent::new(
            project.clone(),
            router.clone(),
            aid.clone(),
            commit.clone(),
            "instructions".into(),
        )
        .await
        .unwrap();

        assert!(!project.agent(&aid).join("state.json").exists());
    }
}

impl AgentState {
    /// init a primary agent from scratch
    fn new(
        commit: String,
        instructions: String,
        max_depth: u32,
    ) -> Result<Self> {
        let state = Self {
            status: AgentStatus::default(),
            assistant: ASSISTANT_POOL
                .get()
                .unwrap()
                .assistant(&ASSISTANT_POOL.get().unwrap().next_primary())?,
            max_depth,
            context: AgentContext {
                commit,
                history: History::new(instructions),
            },
        };
        Ok(state)
    }

    pub async fn save(
        &self,
        project: &Project,
        id: &AgentId,
    ) -> Result<()> {
        project.store().save_agent(id, self).await
    }
}

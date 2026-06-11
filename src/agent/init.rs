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
use crate::llm::provider::assistant::Assistant;
use crate::project::Project;

const CHANNEL_CAPACITY: usize = 100;

impl Agent {
    pub fn new(
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
        let agent = Self::new(
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

impl AgentState {
    /// init a primary agent from scratch
    pub fn new(
        assistant: Assistant,
        commit: String,
        instructions: String,
        max_depth: u32,
    ) -> Self {
        Self {
            status: AgentStatus::default(),
            assistant,
            max_depth,
            context: AgentContext {
                commit,
                history: History::new(instructions),
            },
        }
    }

    pub async fn save(
        &self,
        project: &Project,
        id: &AgentId,
    ) -> Result<()> {
        project.store().save_agent(id, self).await
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::agent::router::AgentRouter;
    use crate::project::layout::LayoutTrait;

    #[tokio::test]
    async fn try_duplicate_registers_child_with_router() {
        let project = Project::new_test().unwrap().0;
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

        let mut state = AgentState::fake(&project);
        state.context.commit = commit.clone();
        let parent = Agent::new(project.clone(), router.clone(), parent_aid.clone(), state);

        let child_aid = router.allocate_agent_id().await.unwrap();
        parent.try_duplicate(child_aid.clone()).await.unwrap();

        // observable via router: deletion succeeds only if registered
        router.delete(child_aid).await.unwrap();
    }
}

pub mod compact;
pub mod handle;
pub mod id;
pub mod init;
#[cfg(test)]
mod loop_tests;
pub mod router;
pub mod run;
pub mod subagent;
pub mod task;
pub mod tool;
pub mod turn;

use derive_more::Display;
pub use id::*;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;

use crate::agent::handle::AgentEvent;
use crate::agent::handle::ParentEvent;
use crate::agent::handle::TurnResult;
use crate::agent::router::AgentRouterHandle;
use crate::agent::task::manager::AgentTaskManager;
use crate::agent::tool::registry::ToolRegistry;
use crate::forward;
use crate::llm::history::History;
use crate::llm::history::TurnStatus;
use crate::llm::provider::assistant::Assistant;
use crate::llm::provider::assistant::AssistantPool;
use crate::project::Project;

#[derive(Debug)]
pub struct Agent {
    pub project: Project,
    pub id: AgentId,
    pub state: AgentState,
    /// router handle for spawning/submitting siblings and children
    pub router: AgentRouterHandle,
    /// pending oneshot for the current turn (set by `Submit` callers that want completion)
    pub pending_done: Option<oneshot::Sender<TurnResult>>,
    // agent event loop
    pub tx: Sender<AgentEvent>,
    pub rx: Receiver<AgentEvent>,
    /// manages jobs in the agent event loop
    pub tskmgr: AgentTaskManager,
    pub tools: ToolRegistry,
}

#[derive(Clone, Serialize, Debug)]
pub struct AgentState {
    /// last emitted status for deduplication of status updates
    #[serde(skip)]
    pub status: AgentStatus,
    pub assistant: Assistant,
    /// Remaining subagent-spawn budget. 0 means this agent cannot spawn
    /// subagents; the subagent tool is filtered out at construction.
    pub max_depth: u32,
    pub context: AgentContext,
}

/// field mirror of `AgentState` for deserialization: the persisted assistant
/// id resolves to an `Assistant` through the pool; `deny_unknown_fields`
/// catches fields added to `AgentState` but not mirrored here
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawAgentState {
    assistant: String,
    max_depth: u32,
    pub context: AgentContext,
}

impl RawAgentState {
    pub fn resolve(
        self,
        assistants: &AssistantPool,
    ) -> anyhow::Result<AgentState> {
        Ok(AgentState {
            status: AgentStatus::default(),
            assistant: assistants.assistant(&self.assistant)?,
            max_depth: self.max_depth,
            context: self.context,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Display)]
pub enum AgentStatus {
    Normal(TurnStatus),
    #[display("compacting: {_0}")]
    Compact(TurnStatus),
}

impl Default for AgentStatus {
    fn default() -> Self {
        Self::Normal(TurnStatus::Idle)
    }
}

impl AgentStatus {
    pub fn turn(&self) -> &TurnStatus {
        match self {
            Self::Normal(t) | Self::Compact(t) => t,
        }
    }

    pub fn idle(&self) -> bool {
        !matches!(self.turn(), TurnStatus::InProgress)
    }

    pub fn label(&self) -> &'static str {
        match self.turn() {
            TurnStatus::InProgress => "+",
            TurnStatus::Idle => " ",
            TurnStatus::Failed(_) => "!",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentContext {
    pub commit: String,
    pub history: History,
}

impl Agent {
    forward! {
        history: History = self.state.context.history;
    }

    pub async fn emit(
        &self,
        event: ParentEvent,
    ) -> anyhow::Result<()> {
        self.router
            .app_tx()
            .send(crate::tui::app::AppEvent::ParentEvent(
                self.id.clone(),
                event,
            ))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod fixtures {
    use std::sync::Arc;

    use super::*;
    use crate::llm::provider::api::fake::FakeApi;
    use crate::project::layout::LayoutTrait;
    use crate::tui::app::AppEvent;

    impl AgentState {
        /// state resolved from the project's fake pool
        pub fn fake(project: &Project) -> Self {
            Self::new(
                project.assistants().assistant("test").unwrap(),
                "".into(),
                "".into(),
                1,
            )
        }
    }

    impl Agent {
        /// agent on a fresh test project, with the pool's scripted api; keep
        /// the receiver alive so `emit` doesn't fail on a closed app channel
        pub async fn fake(name: &str) -> (Self, Arc<FakeApi>, Receiver<AppEvent>) {
            let (project, api) = Project::new_test().unwrap();
            let aid = AgentId::from(format!("{name}-{}", uuid::Uuid::new_v4()));
            tokio::fs::create_dir_all(project.agent(&aid))
                .await
                .unwrap();
            let (app_tx, app_rx) = tokio::sync::mpsc::channel(256);
            let state = AgentState::fake(&project);
            let agent = Self::new(
                project,
                router::AgentRouter::test_handle_with_app_tx(app_tx),
                aid,
                state,
            );
            (agent, api, app_rx)
        }
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;

    #[test]
    fn status_is_not_persisted() {
        let mut state = AgentState::new(Assistant::fake().0, "".into(), "".into(), 1);
        state.status = AgentStatus::Normal(TurnStatus::Failed("oops".into()));

        let serialized = serde_json::to_value(&state).unwrap();
        assert!(serialized.get("status").is_none());

        let restored = serde_json::from_value::<RawAgentState>(serialized)
            .unwrap()
            .resolve(&AssistantPool::fake().0)
            .unwrap();
        assert_eq!(restored.status, AgentStatus::default());
    }
}

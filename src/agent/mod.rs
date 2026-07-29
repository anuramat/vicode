pub mod core;
pub mod handle;
pub mod id;
pub mod init;
mod loop_tests;
mod purity_tests;
pub mod router;
pub mod run;
pub mod task;
pub mod tool;
pub mod turn;

use std::collections::HashMap;

use derive_more::Display;
pub use id::*;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;

use crate::agent::core::AgentCore;
use crate::agent::handle::AgentEvent;
use crate::agent::handle::ParentEvent;
use crate::agent::router::AgentRouterHandle;
use crate::agent::task::executor::TaskExecutor;
use crate::agent::task::sink::OutputChunk;
use crate::llm::history::History;
use crate::llm::history::TurnStatus;
use crate::llm::provider::assistant::Assistant;
use crate::project::Project;

#[derive(Debug)]
pub struct Agent {
    pub id: AgentId,
    pub project: Project,

    /// pure decision logic and agent state
    pub core: AgentCore,
    /// router handle for reaching the app and other agents
    pub router: AgentRouterHandle,
    // agent event loop: inter-agent + task mailbox
    pub tx: Sender<AgentEvent>,
    pub rx: Receiver<AgentEvent>,
    /// app-originated events, drained with priority by the run loop
    pub user_tx: Sender<AgentEvent>,
    pub user_rx: Receiver<AgentEvent>,
    /// router deliveries processed so far
    pub processed: u64,
    /// runs the core's task effects on tokio tasks
    pub executor: TaskExecutor,
    /// dedicated tool-output channel
    pub out_tx: Sender<OutputChunk>,
    pub out_rx: Receiver<OutputChunk>,
    /// per-call streamed output, keyed by `call_id`; outlives the tool future, so abort/panic finalize from it
    pub accumulators: HashMap<String, String>,
    /// the in-flight `DuplicateRequest` ack, armed by `Agent` before the
    /// core runs (the core stays channel-free) and fired by a successful
    /// `Effect::Duplicate`; any other outcome — busy rejection, failure,
    /// panic — drops it, resolving the app's receiver as the total failure
    /// signal
    pub dup_ack: Option<tokio::sync::oneshot::Sender<()>>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentState {
    /// last emitted status for deduplication of status updates
    #[serde(skip)]
    pub status: ActivityStatus,
    /// TODO rename to assistant_id?
    pub assistant: String,
    pub context: AgentContext,
    /// inbound messages buffered while busy/compacting, delivered at the
    /// next true idle; persisted, so a buffered message survives restart
    pub pending_messages: Vec<crate::llm::history::message::UserMessage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Display)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum ActivityStatus {
    Normal(TurnStatus),
    #[display("compacting: {_0}")]
    Compact(TurnStatus),
}

impl Default for ActivityStatus {
    fn default() -> Self {
        Self::Normal(TurnStatus::Idle)
    }
}

impl ActivityStatus {
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
    /// snapshot commit used as lowerdir
    pub commit: String,
    /// diff base
    pub base: String,
    pub history: History,
}

impl Agent {
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
mod tests {
    use std::sync::Arc;

    use similar_asserts::assert_eq;

    use super::*;
    use crate::llm::provider::api::fake::FakeApi;
    use crate::tui::app::AppEvent;

    impl AgentState {
        /// state on the fake pool's `"test"` assistant
        pub fn fake() -> Self {
            Self::new("test".into(), "".into(), "".into())
        }
    }

    impl Agent {
        /// agent on a fresh test project with a real router (so status pings
        /// land in a live graph), on the pool's scripted api; keep the
        /// receiver alive so `emit` doesn't fail on a closed app channel
        pub async fn fake(name: &str) -> (Self, Arc<FakeApi>, Receiver<AppEvent>) {
            let (project, api) = Project::new_test().unwrap();
            let aid = AgentId::from(format!("{name}-{}", uuid::Uuid::new_v4()));
            tokio::fs::create_dir_all(project.agent(&aid))
                .await
                .unwrap();
            let (app_tx, app_rx) = tokio::sync::mpsc::channel(256);
            let router = router::AgentRouter::spawn(
                app_tx,
                project.clone(),
                Default::default(),
                Default::default(),
                Default::default(),
            );
            let state = project.fake_state();
            let agent = Self::new(project, router, aid, state);
            (agent, api, app_rx)
        }
    }

    #[test]
    fn status_is_not_persisted() {
        let mut state = AgentState::fake();
        state.status = ActivityStatus::Normal(TurnStatus::Failed("oops".into()));

        let serialized = serde_json::to_value(&state).unwrap();
        assert!(serialized.get("status").is_none());

        let restored: AgentState = serde_json::from_value(serialized).unwrap();
        assert_eq!(restored.status, ActivityStatus::default());
    }
}

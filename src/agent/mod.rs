pub mod core;
pub mod event;
pub mod id;
pub mod init;
mod loop_tests;
mod purity_tests;
pub mod router;
pub mod run;
pub mod shell;
pub mod task;
pub mod tool;
pub mod turn;

use derive_more::Display;
pub use id::*;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::agent::core::AgentCore;
use crate::agent::event::AgentEvent;
use crate::agent::event::UiEvent;
use crate::agent::event::UserCommand;
use crate::agent::router::Router;
use crate::agent::task::executor::TaskExecutor;
use crate::llm::history::History;
use crate::llm::history::TurnStatus;
use crate::llm::provider::assistant::Assistant;
use crate::project::Project;
use crate::tui::app::AppEvent;

#[derive(Debug)]
pub struct Agent {
    pub id: AgentId,
    pub project: Project,

    pub core: AgentCore,
    /// router handle for reaching other agents
    pub router: Router,
    /// history/status/output updates for rendering
    pub app_tx: Sender<AppEvent>,
    /// ui actions
    pub user_tx: UnboundedSender<UserCommand>,
    pub user_rx: UnboundedReceiver<UserCommand>,
    /// runs the core's task effects on tokio tasks
    pub executor: TaskExecutor,
    /// turn streams and tool calls
    pub task_tx: Sender<AgentEvent>,
    pub task_rx: Receiver<AgentEvent>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentState {
    /// last emitted status for deduplication of status updates
    #[serde(skip)]
    pub status: ActivityStatus,
    pub assistant_id: String,
    pub context: AgentContext,
    /// inbound messages buffered while busy
    pub pending_messages: Vec<crate::llm::history::message::UserMessage>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Display)]
#[cfg_attr(test, derive(serde::Serialize))]
#[display("{turn}{}", if *compacting { ", compacting" } else { "" })]
pub struct ActivityStatus {
    pub turn: TurnStatus,
    /// a summary is in flight, or waiting for the turn to end
    pub compacting: bool,
}

impl ActivityStatus {
    pub fn idle(&self) -> bool {
        !matches!(self.turn, TurnStatus::InProgress) && !self.compacting
    }

    pub fn label(&self) -> &'static str {
        match self.turn {
            TurnStatus::Failed(_) => "!",
            _ if !self.idle() => "+",
            _ => " ",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentContext {
    /// the tab's snapshot commit: the overlay lowerdir, shared by every
    /// agent in the tab
    pub commit: String,
    pub history: History,
}

impl Agent {
    pub async fn emit(
        &self,
        event: UiEvent,
    ) -> anyhow::Result<()> {
        self.app_tx
            .send(AppEvent::Agent(self.id.clone(), event))
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
        /// agent on a fresh test project with a real router (so status reports
        /// land in a live graph), on the pool's scripted api; keep the
        /// receiver alive so `emit` doesn't fail on a closed app channel
        pub async fn fake(name: &str) -> (Self, Arc<FakeApi>, Receiver<AppEvent>) {
            let (project, api) = Project::new_test().unwrap();
            let aid = AgentId::from(format!("{name}-{}", uuid::Uuid::new_v4()));
            tokio::fs::create_dir_all(project.agent(&aid))
                .await
                .unwrap();
            let (app_tx, app_rx) = tokio::sync::mpsc::channel(256);
            let router = router::RouterState::start(
                app_tx.clone(),
                project.clone(),
                Default::default(),
                Default::default(),
                Default::default(),
            );
            let state = project.fake_state();
            let agent = Self::new(project, router, app_tx, aid, state);
            (agent, api, app_rx)
        }
    }

    #[test]
    fn status_is_not_persisted() {
        let mut state = AgentState::fake();
        state.status = ActivityStatus {
            turn: TurnStatus::Failed("oops".into()),
            compacting: true,
        };

        let serialized = serde_json::to_value(&state).unwrap();
        assert!(serialized.get("status").is_none());

        let restored: AgentState = serde_json::from_value(serialized).unwrap();
        assert_eq!(restored.status, ActivityStatus::default());
    }
}

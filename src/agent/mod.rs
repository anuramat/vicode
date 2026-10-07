pub mod event;
pub mod handle;
pub mod id;
pub mod init;
mod loop_tests;
pub mod router;
pub mod run;
pub mod task;
pub mod tool;
pub mod turn;

use derive_more::From;
pub use id::*;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::agent::event::AgentEvent;
use crate::agent::event::UiEvent;
use crate::agent::event::UserCommand;
use crate::agent::router::Router;
use crate::agent::task::executor::TaskExecutor;
use crate::agent::task::ledger::TaskLedger;
use crate::forward;
use crate::llm::history::Compaction;
use crate::llm::history::History;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::PeerMessage;
use crate::llm::history::message::UserMessage;
use crate::llm::provider::assistant::Assistant;
use crate::project::Project;
use crate::tui::app::AppEvent;

#[derive(Debug)]
pub struct Agent {
    pub project: Project,

    pub id: AgentId,
    pub state: AgentState,
    /// state has changed since last persisted
    dirty: bool,

    pub needs_turn: bool,
    pub executor: TaskExecutor,
    pub ledger: TaskLedger,
    pub compaction: Option<Compaction>,

    /// other agents
    pub router: Router,
    /// ui updates
    pub app_tx: UnboundedSender<AppEvent>,
    // ui actions
    pub user_tx: UnboundedSender<UserCommand>,
    pub user_rx: UnboundedReceiver<UserCommand>,
    // turn streams and tool calls
    pub task_tx: UnboundedSender<AgentEvent>,
    pub task_rx: UnboundedReceiver<AgentEvent>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentState {
    pub assistant_id: String,
    pub context: AgentContext,
    /// inbound messages buffered while busy
    pub pending_messages: Vec<PendingMessage>,
}

/// an inbound message, buffered until the next turn boundary
#[derive(Clone, Serialize, Deserialize, Debug, From)]
pub enum PendingMessage {
    User(UserMessage),
    Peer(PeerMessage),
}

impl From<PendingMessage> for HistoryUpdate {
    fn from(msg: PendingMessage) -> Self {
        match msg {
            PendingMessage::User(m) => Self::UserMessage(m),
            PendingMessage::Peer(m) => Self::DeveloperMessage(DeveloperMessage::Peer(m)),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentContext {
    // TODO this is overlay specific, we should probably move it somewhere else
    /// snapshot commit that the overlay uses as the lowerdir
    pub commit: String,
    pub history: History,
}

impl Agent {
    forward! {
        history: History = self.state.context.history;
    }

    /// a closed app bus means the app is shutting down: nothing to report to
    pub fn emit(
        &self,
        event: UiEvent,
    ) {
        drop(self.app_tx.send(AppEvent::Agent(self.id.clone(), event)));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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
        /// land in a live graph), on the pool's scripted api; the receiver
        /// gets the agent's UI events
        pub async fn fake(name: &str) -> (Self, Arc<FakeApi>, UnboundedReceiver<AppEvent>) {
            let (project, api) = Project::new_test().unwrap();
            let aid = AgentId::from(format!("{name}-{}", uuid::Uuid::new_v4()));
            tokio::fs::create_dir_all(project.agent(&aid))
                .await
                .unwrap();
            let (app_tx, app_rx) = tokio::sync::mpsc::unbounded_channel();
            let router = router::Router::new(app_tx.clone(), project.clone());
            let state = project.fake_state();
            let agent = Self::new(project, router, app_tx, aid, state);
            (agent, api, app_rx)
        }
    }
}

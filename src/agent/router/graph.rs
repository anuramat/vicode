use derive_more::Display;
use futures::future::AbortHandle;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Sender;

use crate::agent::AgentId;
use crate::agent::handle::AgentEvent;
use crate::agent::router::api::TurnOutcome;
use crate::agent::router::api::WaitResult;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphRecord {
    pub root: AgentId,
    pub parent: Option<AgentId>,
    pub archived: bool,
}

#[derive(Debug)]
pub struct StatusReport {
    pub processed: u64,
    pub status: NodeStatus,
    pub outcome: TurnOutcome,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Display, Serialize, Deserialize, schemars::JsonSchema,
)]
pub enum NodeStatus {
    Spawning,
    Running,
    Idle,
    Dead,
}

#[derive(Debug)]
pub struct AgentNode {
    /// inter-agent/task events
    pub mailbox: Option<Sender<AgentEvent>>,
    /// ui events, prioritized over mailbox
    pub user_tx: Option<Sender<AgentEvent>>,
    pub abort: Option<AbortHandle>,

    pub delivered: u64,
    pub processed: u64,

    pub root: AgentId,
    pub parent: Option<AgentId>,
    pub status: NodeStatus,
    /// cache
    pub outcome: TurnOutcome,
}

impl AgentNode {
    pub fn new(
        root: AgentId,
        parent: Option<AgentId>,
        status: NodeStatus,
    ) -> Self {
        Self {
            mailbox: None,
            user_tx: None,
            abort: None,
            delivered: 0,
            processed: 0,
            root,
            parent,
            status,
            outcome: TurnOutcome::default(),
        }
    }

    pub fn record(
        &self,
        archived: bool,
    ) -> GraphRecord {
        GraphRecord {
            root: self.root.clone(),
            parent: self.parent.clone(),
            archived,
        }
    }

    pub fn wait_result(&self) -> WaitResult {
        WaitResult {
            status: self.status,
            outcome: self.outcome.clone(),
        }
    }
}

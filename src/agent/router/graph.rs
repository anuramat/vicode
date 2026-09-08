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
pub struct StatusPing {
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
    /// inter-agent + task mailbox; `None` = unattached/dead
    pub mailbox: Option<Sender<AgentEvent>>,
    /// app-originated events ride a dedicated, priority-polled channel
    pub user_tx: Option<Sender<AgentEvent>>,
    /// out-of-band kill switch, set once the runtime is attached
    pub abort: Option<AbortHandle>,
    /// mailbox deliveries, parks included; a `wait` resolves only
    /// once the agent has processed them all — woken is derived, not a status
    pub delivered: u64,
    /// the agent's processed watermark from its last ping
    pub processed: u64,
    pub root: AgentId,
    pub parent: Option<AgentId>,
    pub status: NodeStatus,
    /// cached last turn's outcome
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

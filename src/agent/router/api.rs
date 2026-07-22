//! the router's model-facing interface

use serde::Deserialize;
use serde::Serialize;

use crate::agent::AgentId;
use crate::agent::router::graph::NodeStatus;

/// router errors that can be caused by inter-agent comms
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RouterError {
    #[error("target unreachable: not a live agent in your tab")]
    Unreachable,
    #[error("target mailbox full; retry later")]
    Busy,
    #[error("this wait would close a wait cycle")]
    WouldDeadlock,
    #[error("target is not a strict spawn-descendant of yours")]
    NotOwned,
}

/// last output text and typed failure
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnOutcome {
    pub output: Option<String>,
    pub error: Option<String>,
}

/// `wait` tool return value
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitResult {
    pub status: NodeStatus,
    #[serde(flatten)]
    pub outcome: TurnOutcome,
}

/// one entry of the `list` tool return value
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListEntry {
    pub id: AgentId,
    pub parent: Option<AgentId>,
    pub status: NodeStatus,
}

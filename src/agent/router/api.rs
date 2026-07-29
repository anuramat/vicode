//! the router's model-facing interface

use serde::Deserialize;
use serde::Serialize;

use crate::agent::AgentId;
use crate::agent::router::graph::NodeStatus;

/// router errors that can be caused by inter-agent comms
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RouterError {
    #[error("target unreachable: not a live agent in your tab")]
    Unreachable,
    #[error("target is dead: {0}")]
    Dead(String),
    #[error("target is not a strict spawn-descendant of yours")]
    NotOwned,
}

/// one entry of the `list` tool return value
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListEntry {
    pub id: AgentId,
    pub parent: Option<AgentId>,
    pub status: NodeStatus,
}

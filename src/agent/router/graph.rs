use futures::future::AbortHandle;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::agent::AgentId;
use crate::agent::event::AgentEvent;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphRecord {
    pub root: AgentId,
    pub parent: Option<AgentId>,
    pub archived: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Running,
    Idle,
    Dead,
}

#[derive(Debug)]
pub struct AgentNode {
    pub root: AgentId,
    pub parent: Option<AgentId>,
    pub runtime: Runtime,
}

#[derive(Debug)]
pub enum Runtime {
    /// `busy` until the runtime's startup report
    Live {
        /// the agent's event channel: mail queues there until the runtime
        /// drains it
        mailbox: UnboundedSender<AgentEvent>,
        abort: AbortHandle,
        busy: bool,
    },
    /// terminal for this process, with the error that ended it
    Dead(String),
}

impl AgentNode {
    pub fn live(
        root: AgentId,
        parent: Option<AgentId>,
        mailbox: UnboundedSender<AgentEvent>,
        abort: AbortHandle,
    ) -> Self {
        Self {
            root,
            parent,
            runtime: Runtime::Live {
                mailbox,
                abort,
                busy: true,
            },
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

    pub fn status(&self) -> NodeStatus {
        match &self.runtime {
            Runtime::Live { busy: true, .. } => NodeStatus::Running,
            Runtime::Live { busy: false, .. } => NodeStatus::Idle,
            Runtime::Dead(_) => NodeStatus::Dead,
        }
    }
}

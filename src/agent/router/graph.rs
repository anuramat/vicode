use futures::future::AbortHandle;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::mpsc::unbounded_channel;

use crate::agent::AgentId;
use crate::llm::history::message::PeerMessage;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphRecord {
    pub root: AgentId,
    pub parent: Option<AgentId>,
    pub archived: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Spawning,
    Running,
    Idle,
    Dead,
}

#[derive(Debug)]
pub struct AgentNode {
    pub root: AgentId,
    pub parent: Option<AgentId>,
    /// minted with the node, so mail sent before the runtime starts parks here
    pub mailbox: UnboundedSender<PeerMessage>,
    pub runtime: Runtime,
}

#[derive(Debug)]
pub enum Runtime {
    /// the mailbox's receiving end, until `launch` hands it to the runtime
    Pending(UnboundedReceiver<PeerMessage>),
    /// `status` stays `Spawning` until the runtime's startup report
    Live {
        abort: AbortHandle,
        status: NodeStatus,
    },
    /// terminal for this process, with the error that ended it
    Dead(String),
}

impl AgentNode {
    pub fn new(
        root: AgentId,
        parent: Option<AgentId>,
    ) -> Self {
        let (mailbox, rx) = unbounded_channel();
        Self {
            root,
            parent,
            mailbox,
            runtime: Runtime::Pending(rx),
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
            Runtime::Pending(_) => NodeStatus::Spawning,
            Runtime::Live { status, .. } => *status,
            Runtime::Dead(_) => NodeStatus::Dead,
        }
    }
}

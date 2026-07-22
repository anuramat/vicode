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

#[derive(Debug)]
pub struct AgentNode {
    pub root: AgentId,
    pub parent: Option<AgentId>,
    pub runtime: Runtime,
}

#[derive(Debug)]
pub enum Runtime {
    Live {
        tx: UnboundedSender<AgentEvent>,
        abort: AbortHandle,
    },
    Dead(String),
}

impl AgentNode {
    pub fn live(
        root: AgentId,
        parent: Option<AgentId>,
        tx: UnboundedSender<AgentEvent>,
        abort: AbortHandle,
    ) -> Self {
        Self {
            root,
            parent,
            runtime: Runtime::Live { tx, abort },
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
}

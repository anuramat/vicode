use derive_more::Display;
use futures::future::AbortHandle;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::mpsc::unbounded_channel;

use crate::agent::AgentId;
use crate::agent::event::Mail;
use crate::agent::router::api::TurnOutcome;
use crate::agent::router::api::WaitResult;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphRecord {
    pub root: AgentId,
    pub parent: Option<AgentId>,
    pub archived: bool,
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
    pub root: AgentId,
    pub parent: Option<AgentId>,
    /// minted with the node, so mail sent before the runtime starts parks here
    pub mailbox: UnboundedSender<Mail>,
    pub runtime: Runtime,
    /// last good output, as last reported: a wait on a dead agent still gets it
    pub output: Option<String>,
}

#[derive(Debug)]
pub enum Runtime {
    /// the mailbox's receiving end, until `launch` hands it to the runtime
    Pending(UnboundedReceiver<Mail>),
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
            output: None,
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

    /// what a wait on a dead agent returns
    pub fn death(&self) -> Option<WaitResult> {
        let Runtime::Dead(error) = &self.runtime else {
            return None;
        };
        Some(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: self.output.clone(),
                error: Some(error.clone()),
            },
        })
    }
}

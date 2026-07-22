use anyhow::Result;
use tokio::sync::oneshot;

use super::RuntimeHandle;
use crate::agent::AgentId;
use crate::agent::handle::ExternalEvent;
use crate::agent::router::api::ListEntry;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::WaitResult;
use crate::agent::router::graph::NodeStatus;
use crate::agent::router::graph::StatusReport;
use crate::llm::history::History;

#[derive(Debug)]
pub enum RouterCommand {
    RegisterRoot {
        aid: AgentId,
        done: oneshot::Sender<Result<()>>,
    },
    Forward {
        aid: AgentId,
        event: ExternalEvent,
        done: oneshot::Sender<Result<()>>,
    },
    Allocate {
        done: oneshot::Sender<AgentId>,
    },
    Status {
        aid: AgentId,
        report: StatusReport,
    },
    AttachRuntime {
        aid: AgentId,
        runtime: RuntimeHandle,
        done: oneshot::Sender<Result<()>>,
    },
    RuntimeDown {
        aid: AgentId,
        error: String,
    },
    Spawn {
        parent: AgentId,
        capture: Option<History>,
        prompt: String,
        done: oneshot::Sender<Result<AgentId>>,
    },
    Send {
        caller: AgentId,
        target: AgentId,
        text: String,
        done: oneshot::Sender<Result<(), RouterError>>,
    },
    Inspect {
        caller: AgentId,
        target: AgentId,
        done: oneshot::Sender<Result<NodeStatus, RouterError>>,
    },
    Wait {
        caller: AgentId,
        target: AgentId,
        done: oneshot::Sender<Result<WaitResult, RouterError>>,
    },
    List {
        caller: AgentId,
        subtree: bool,
        done: oneshot::Sender<Option<Vec<ListEntry>>>,
    },
    Archive {
        caller: AgentId,
        target: AgentId,
        done: oneshot::Sender<Result<(), RouterError>>,
    },
    ArchiveTab {
        primary: AgentId,
        done: oneshot::Sender<Result<()>>,
    },
    RollbackSpawn {
        aid: AgentId,
        done: oneshot::Sender<Result<()>>,
    },
    #[cfg(test)]
    WaitIdle {
        aid: AgentId,
        done: oneshot::Sender<Result<WaitResult, RouterError>>,
    },
    #[cfg(test)]
    Shutdown {
        aid: AgentId,
        done: oneshot::Sender<Result<()>>,
    },
}

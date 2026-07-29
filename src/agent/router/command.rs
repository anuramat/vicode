//! the router's mailbox: one variant per handle request

use anyhow::Result;
use tokio::sync::oneshot;

use super::RuntimeHandle;
use crate::agent::AgentId;
use crate::agent::handle::ExternalEvent;
use crate::agent::router::api::ListEntry;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::WaitResult;
use crate::agent::router::graph::NodeStatus;
use crate::agent::router::graph::StatusPing;
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
    #[cfg(test)]
    Shutdown {
        aid: AgentId,
        done: oneshot::Sender<Result<()>>,
    },
    /// liveness ping from an agent or a detached tail: per status transition
    /// and per processed delivery; the idle ping carries the last assistant
    /// text (see [`StatusPing`])
    Status {
        aid: AgentId,
        ping: StatusPing,
    },
    /// one-shot runtime attachment: a node may never replace its runtime
    AttachRuntime {
        aid: AgentId,
        runtime: RuntimeHandle,
        done: oneshot::Sender<Result<()>>,
    },
    /// terminal report from the runtime supervisor
    RuntimeDown {
        aid: AgentId,
        error: String,
    },
    Spawn {
        parent: AgentId,
        capture: History,
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
    /// tab-scope form: archive every member of the primary's tab
    ArchiveTab {
        primary: AgentId,
        done: oneshot::Sender<Result<()>>,
    },
    /// remove a failed provisional spawn and its durable/filesystem residue
    RollbackSpawn {
        aid: AgentId,
        done: oneshot::Sender<Result<()>>,
    },
    #[cfg(test)]
    WaitIdle {
        aid: AgentId,
        done: oneshot::Sender<Result<WaitResult, RouterError>>,
    },
}

//! agent wire types: what the `Agent` consumes and what it tells the UI

use anyhow::Result;

use crate::agent::ActivityStatus;
use crate::agent::id::AgentId;
use crate::agent::task::ledger::TaskId;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::ToolCallItem;
use crate::llm::history::message::UserMessage;

#[derive(Debug)]
pub enum AgentEvent {
    TaskDone(TaskId, Result<()>),
    TaskEvent(TaskId, HistoryGeneration, HistoryUpdate),
    /// reaper: tool future returned its resolved item
    ToolResolved(TaskId, Box<ToolCallItem>),
    /// reaper: tool future panicked/cancelled; error = marker + partial output
    ToolFailed {
        id: TaskId,
        call_id: String,
        error: String,
    },
    /// inter-agent message (router `send` / spawn seed): wakes a turn if
    /// idle, buffers if busy
    Inbound(UserMessage),
    User(UserCommand),
}

#[derive(Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum UiEvent {
    Started(Box<crate::agent::AgentState>),
    HistoryUpdate(HistoryGeneration, HistoryUpdate),
    StatusUpdate(ActivityStatus),
    AssistantSet(String),
    Error(String),
    /// live tool-output chunk for rendering; the `Agent` accumulator is
    /// authoritative, so a dropped chunk costs a render frame only
    ToolOutput {
        call_id: String,
        chunk: String,
    },
}

#[derive(Debug)]
pub enum UserCommand {
    /// Compact the first n messages
    Compact(usize),
    Retry,
    Abort,
    Undo(usize), // TODO maybe this should send generation or whatever
    SetAssistant(String),
    Submit(UserPrompt),
    DuplicateRequest {
        copy: AgentId,
        /// resolved iff the copy registered; dropped-channel semantics make
        /// the failure signal total — busy rejection, unknown aid, dead
        /// `Agent`, and panic all close the app's receiver
        ack: tokio::sync::oneshot::Sender<()>,
    },
}

#[derive(Debug)]
pub struct UserPrompt {
    pub text: String,
    /// None = the receiving agent uses its current one
    pub generation: Option<HistoryGeneration>,
}

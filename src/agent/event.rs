//! agent wire types: what the `Agent` consumes and what it tells the UI

use crate::agent::ActivityStatus;
use crate::agent::id::AgentId;
use crate::agent::task::ledger::TaskId;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::ToolCallItem;
use crate::llm::history::message::UserMessage;

/// everything the core consumes; task events are interpreted against the
/// core's ledger, so they carry nothing but the task id
#[derive(Debug)]
pub enum AgentEvent {
    User(UserCommand),
    /// inter-agent message (router `send` / spawn seed): wakes a turn if
    /// idle, buffers if busy
    Inbound(UserMessage),
    /// a turn's provider stream
    Stream(TaskId, AssistantEvent),
    /// a streaming tool's output chunk
    Output(TaskId, String),
    /// a task's terminal: return, error or panic. Cancelled tasks never
    /// report — abort resolves them in the core
    Done(TaskId, TaskResult),
}

/// a turn resolves to `None`, a tool to its item; `Err` = turn error or panic
pub type TaskResult = Result<Option<Box<ToolCallItem>>, String>;

#[derive(Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum UiEvent {
    Started(Box<crate::agent::AgentState>),
    HistoryUpdate(HistoryGeneration, HistoryUpdate),
    StatusUpdate(ActivityStatus),
    AssistantSet(String),
    Error(String),
    /// live tool-output chunk for rendering; the core's ledger is
    /// authoritative, so a dropped chunk costs a render frame only
    ToolOutput {
        call_id: String,
        chunk: String,
    },
    /// a `Duplicate` didn't produce the copy: the app drops its preview tab
    DuplicateFailed {
        copy: AgentId,
        error: String,
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
    /// clone into a new root under the given (allocated) id
    Duplicate(AgentId),
}

#[derive(Debug)]
pub struct UserPrompt {
    pub text: String,
    /// None = the receiving agent uses its current one
    pub generation: Option<HistoryGeneration>,
}

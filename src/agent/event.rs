//! agent wire types: what the `Agent` consumes and what it tells the UI

use tokio::sync::mpsc::UnboundedSender;

use crate::agent::ActivityStatus;
use crate::agent::id::AgentId;
use crate::agent::router::api::WaitId;
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
    Mail(Mail),
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

/// router-delivered; the mailbox is FIFO, which is the whole delivery
/// protocol: a `Wait` is answered only after every message its caller sent
/// before it
#[derive(Debug)]
pub enum Mail {
    /// inter-agent message (router `send` / spawn seed): wakes a turn if
    /// idle, buffers if busy
    Message(UserMessage),
    /// a registered `wait`: settled as soon as this agent is idle
    Wait(WaitId),
}

#[derive(Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum UiEvent {
    /// the runtime is up: the app's tab takes the state and the control lane
    Started {
        state: Box<crate::agent::AgentState>,
        #[cfg_attr(test, serde(skip))]
        control: UnboundedSender<UserCommand>,
    },
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

use tokio::sync::mpsc::UnboundedSender;

use crate::agent::ActivityStatus;
use crate::agent::id::AgentId;
use crate::agent::task::ledger::TaskId;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::ToolCallItem;
use crate::llm::history::message::UserMessage;

#[derive(Debug)]
pub enum AgentEvent {
    User(UserCommand),
    /// inter-agent message
    Message(UserMessage),
    /// a turn's provider stream
    Stream(TaskId, AssistantEvent),
    /// a streaming tool's output chunk
    Output(TaskId, String),
    /// a task's terminal: return, error or panic
    Done(TaskId, TaskResult),
}

/// a turn resolves to `None`, a tool to its item; `Err` = turn error or panic
pub type TaskResult = Result<Option<Box<ToolCallItem>>, String>;

#[derive(Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum UiEvent {
    Started {
        state: Box<crate::agent::AgentState>,
        #[cfg_attr(test, serde(skip))]
        control: UnboundedSender<UserCommand>,
    },
    HistoryUpdate(HistoryGeneration, HistoryUpdate),
    StatusUpdate(ActivityStatus),
    AssistantSet(String),
    Error(String),
    ToolOutput {
        call_id: String,
        chunk: String,
    },
    DuplicateFailed {
        copy: AgentId,
        error: String,
    },
}

#[derive(Debug)]
pub enum UserCommand {
    /// compact the first n messages
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

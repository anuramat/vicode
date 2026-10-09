use tokio::sync::mpsc::UnboundedSender;

use crate::agent::id::AgentId;
use crate::agent::task::TaskId;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::CompactMessage;
use crate::llm::history::message::PeerMessage;
use crate::llm::history::message::ToolCallItem;

#[derive(Debug)]
pub enum AgentEvent {
    /// from the agent's tab
    User(UserCommand),
    /// inter-agent message, sent by the router into the agent's channel
    Message(PeerMessage),
    /// a turn's provider stream
    Stream(TaskId, AssistantEvent),
    /// a streaming tool's output chunk
    Output(TaskId, String),
    /// a task's terminal: return, error or panic
    Done(TaskId, TaskResult),
}

/// `Err` = task error or panic
pub type TaskResult = Result<TaskOutput, String>;

#[derive(Debug)]
pub enum TaskOutput {
    /// a turn's stream already landed in history
    Turn,
    Tool(Box<ToolCallItem>),
    Summary(CompactMessage),
}

#[derive(Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum UiEvent {
    Started {
        state: Box<crate::agent::AgentState>,
        #[cfg_attr(test, serde(skip))]
        control: UnboundedSender<AgentEvent>,
    },
    HistoryUpdate(HistoryGeneration, HistoryUpdate),
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
    /// summarize the first n messages in the background
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
    pub generation: HistoryGeneration,
}

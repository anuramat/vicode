use tokio::sync::mpsc::UnboundedSender;

use crate::agent::id::AgentId;
use crate::agent::task::TaskId;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::CompactMessage;
use crate::llm::history::message::PeerMessage;
use crate::llm::history::message::ToolCallItem;

#[derive(Debug)]
pub enum AgentEvent {
    /// from the agent's tab
    User(UserCommand),
    /// inter-agent message
    Message(PeerMessage),
    /// provider stream
    Stream(TaskId, AssistantEvent),
    /// tool output chunk
    Output(TaskId, String),
    /// task completed
    Done(TaskId, TaskResult),
}

pub type TaskResult = Result<TaskOutput, String>;

#[derive(Debug)]
pub enum TaskOutput {
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
    HistoryUpdate(HistoryUpdate),
    AssistantSet(String),
    Error(String),
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
    Undo(usize), // TODO guard against popping messages the user hasn't seen yet
    SetAssistant(String),
    Submit(String),
    /// clone into a new root under the given (allocated) id
    Duplicate(AgentId),
}

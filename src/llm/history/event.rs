use crate::llm::history::compact::Compaction;
use crate::llm::history::delta::Delta;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::UserMessage;
use crate::llm::history::timing::now;

#[derive(Debug, Clone)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum AssistantEvent {
    Created { created_at: u64 },
    Started { started_at: u64 },
    Delta(Delta),
    Item(Box<AssistantItem>),
    Completed { ended_at: u64 },
    Failed { message: String, ended_at: u64 },
}

#[derive(Debug, Clone)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum HistoryUpdate {
    /// replace the oldest messages with their summary
    Compact(Compaction),
    TurnResponse(AssistantEvent),
    UserMessage(UserMessage),
    DeveloperMessage(DeveloperMessage),
    Pop(usize),
    // TODO expand etc
    /// a tool task failed
    ToolCallFailed {
        call_id: String,
        error: String,
    },
    /// a chunk of an in-flight tool call's output
    ToolOutput {
        call_id: String,
        chunk: String,
    },
    /// everything in flight is gone: fail the response and every unresolved
    /// tool call
    FailUnresolved {
        message: String,
        ended_at: u64,
    },
}

impl AssistantEvent {
    pub fn created() -> Self {
        Self::Created { created_at: now() }
    }

    pub fn completed() -> Self {
        Self::Completed { ended_at: now() }
    }

    /// item is final: stamps `ended_at`
    pub fn item_done(mut item: AssistantItem) -> Self {
        item.touch_ended_at(now());
        Self::Item(Box::new(item))
    }
}

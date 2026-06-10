use crate::llm::history::compact::CompactStart;
use crate::llm::history::delta::Delta;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::UserMessage;
use crate::llm::history::timing::now;

pub type HistoryGeneration = u64;

#[derive(Debug, Clone)]
pub enum AssistantEvent {
    Created { created_at: u64 },
    Started { started_at: u64 },
    Delta(Delta),
    Item(Box<AssistantItem>),
    Completed { ended_at: u64 },
    Failed { message: String, ended_at: u64 },
}

#[derive(Debug, Clone)]
pub enum HistoryUpdate {
    CompactStart(CompactStart),
    CompactAbort,
    CompactResponse(AssistantEvent),
    GenerationIncremented,
    TurnResponse(AssistantEvent),
    UserMessage(UserMessage),
    DeveloperMessage(DeveloperMessage),
    Pop(usize),
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

    pub fn failed(message: String) -> Self {
        Self::Failed {
            message,
            ended_at: now(),
        }
    }
}

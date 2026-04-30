use crate::llm::history::delta::Delta;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::timing::now;

pub type HistoryGeneration = u64;

#[derive(Debug, Clone)]
pub enum AssistantEvent {
    Created(u64),
    Started(u64),
    Delta(Delta),
    Item(Box<AssistantItem>),
    Completed(Vec<AssistantItem>),
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum HistoryUpdate {
    CompactStart { n_drop: usize },
    CompactAbort,
    CompactResponse(AssistantEvent),
    GenerationIncremented,
    TurnResponse(AssistantEvent),
    UserMessage(String),
    DeveloperMessage(DeveloperMessage),
    Pop(usize),
}

impl AssistantEvent {
    pub fn created() -> Self {
        Self::Created(now())
    }
}

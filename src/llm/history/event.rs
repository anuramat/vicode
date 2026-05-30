use crate::llm::history::delta::Delta;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::UserMessage;
use crate::llm::history::timing::now;

pub type HistoryGeneration = u64;

const COMPACT_PROMPT: &str = "Summarize this conversation for future continuation. Keep concrete user requirements, decisions, constraints, file paths, and unresolved work. Be concise and factual. Output plain text only.";

#[derive(Debug, Clone)]
pub enum AssistantEvent {
    Created(u64),
    Started(u64),
    Delta(Delta),
    Item(Box<AssistantItem>),
    Completed {
        items: Vec<AssistantItem>,
        ended_at: u64,
    },
    Failed {
        message: String,
        ended_at: u64,
    },
}

#[derive(Debug, Clone)]
pub struct CompactStart {
    pub n_drop: usize,
    pub created_at: u64,
    pub prompt: UserMessage,
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
        Self::Created(now())
    }

    pub fn completed(items: Vec<AssistantItem>) -> Self {
        Self::Completed {
            items,
            ended_at: now(),
        }
    }

    pub fn failed(message: String) -> Self {
        Self::Failed {
            message,
            ended_at: now(),
        }
    }
}

impl CompactStart {
    pub fn new(n_drop: usize) -> Self {
        let created_at = now();
        Self {
            n_drop,
            created_at,
            prompt: UserMessage::new_at(COMPACT_PROMPT.into(), created_at),
        }
    }
}

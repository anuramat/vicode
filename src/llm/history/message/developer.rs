use ambassador::Delegate;
use ambassador::delegatable_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::llm::history::timing::Timing;
use crate::llm::history::timing::ambassador_impl_Timing;
use crate::llm::history::timing::now;
use crate::llm::history::tokens::TokenCount;
use crate::llm::history::tokens::count_text_tokens;

/// convert to assistant-readable text
#[delegatable_trait]
pub trait AsMessageText {
    fn as_message_text(&self) -> &str;
}

#[derive(Clone, Serialize, Deserialize, Debug, Delegate)]
#[delegate(AsMessageText)]
#[delegate(Timing)]
pub enum DeveloperMessage {
    Compact(CompactMessage),
    Misc(MiscMessage),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct CompactMessage {
    pub text: String,
    pub needs_another_turn: bool,

    pub token_count: usize,

    pub created_at: u64,
    pub started_at: u64,
    pub ended_at: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct MiscMessage {
    text: String,
    token_count: usize,
    created_at: u64,
}

impl DeveloperMessage {
    pub fn misc(text: String) -> Self {
        let mut result = Self::Misc(MiscMessage {
            text,
            created_at: now(),
            token_count: 0,
        });
        result.recount();
        result
    }
}

impl TokenCount for DeveloperMessage {
    fn recount(&mut self) {
        let token_count = count_text_tokens(self.as_message_text());
        match self {
            Self::Compact(msg) => msg.token_count = token_count,
            Self::Misc(msg) => msg.token_count = token_count,
        }
    }

    fn token_count(&self) -> usize {
        match self {
            Self::Compact(msg) => msg.token_count,
            Self::Misc(msg) => msg.token_count,
        }
    }
}

impl AsMessageText for CompactMessage {
    fn as_message_text(&self) -> &str {
        &self.text
    }
}

impl AsMessageText for MiscMessage {
    fn as_message_text(&self) -> &str {
        &self.text
    }
}

impl Timing for CompactMessage {
    fn created_at(&self) -> u64 {
        self.created_at
    }

    fn started_at(&self) -> Option<u64> {
        Some(self.started_at)
    }

    fn ended_at(&self) -> Option<u64> {
        Some(self.ended_at)
    }
}

impl Timing for MiscMessage {
    fn created_at(&self) -> u64 {
        self.created_at
    }
}

mod user;
use ambassador::Delegate;
pub use assistant::*;
use derive_more::From;
pub use developer::*;
use serde::Deserialize;
use serde::Serialize;
use strum::EnumTryAs;
pub use user::UserMessage;

pub mod assistant;
pub mod developer;

use crate::llm::history::timing::Timing;
use crate::llm::history::timing::ambassador_impl_Timing;
use crate::llm::history::tokens::MESSAGE_OVERHEAD;
use crate::llm::history::tokens::TokenCount;

#[derive(Clone, Serialize, Deserialize, Debug, From, Delegate, EnumTryAs)]
#[delegate(Timing)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    Developer(DeveloperMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
}

impl TokenCount for Message {
    fn recount(&mut self) {
        match self {
            Self::Developer(m) => m.recount(),
            Self::User(m) => m.recount(),
            Self::Assistant(m) => m.recount(),
        }
    }

    fn token_count(&self) -> usize {
        (match self {
            Self::Developer(m) => m.token_count(),
            Self::User(m) => m.token_count(),
            Self::Assistant(m) => m.token_count(),
        }) + MESSAGE_OVERHEAD
    }
}

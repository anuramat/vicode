use serde::Deserialize;
use serde::Serialize;

use crate::llm::history::timing::Timing;
use crate::llm::history::timing::now;
use crate::llm::history::tokens::TokenCount;
use crate::llm::history::tokens::count_text_tokens;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct UserMessage {
    pub text: String,
    pub token_count: usize,
    pub created_at: u64,
}

impl UserMessage {
    pub fn new(text: String) -> Self {
        let mut result = Self {
            text,
            created_at: now(),
            token_count: 0,
        };
        result.recount();
        result
    }
}

impl Timing for UserMessage {
    fn created_at(&self) -> u64 {
        self.created_at
    }
}

impl TokenCount for UserMessage {
    fn recount(&mut self) {
        self.token_count = count_text_tokens(&self.text);
    }

    fn token_count(&self) -> usize {
        self.token_count
    }
}

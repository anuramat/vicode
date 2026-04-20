use serde::Deserialize;
use serde::Serialize;

use crate::llm::history::timing::Timing;
use crate::llm::history::timing::now;
use crate::llm::history::tokens::TokenCount;
use crate::llm::history::tokens::count_text_tokens;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ReasoningItem {
    pub id: String,
    pub content: Option<Vec<String>>,
    pub summary: Vec<String>,
    pub encrypted: Option<String>,

    pub token_count: usize, // TODO make an Option

    pub started_at: u64,
    pub ended_at: Option<u64>,
}

impl ReasoningItem {
    pub fn new(id: String) -> Self {
        Self {
            id,
            started_at: now(),
            ended_at: None,
            token_count: 0,
            content: None,
            summary: Vec::new(),
            encrypted: None,
        }
    }
}

impl Timing for ReasoningItem {
    fn created_at(&self) -> u64 {
        self.started_at
    }

    fn started_at(&self) -> Option<u64> {
        Some(self.started_at)
    }

    fn ended_at(&self) -> Option<u64> {
        self.ended_at
    }
}

impl TokenCount for ReasoningItem {
    fn recount(&mut self) {
        self.token_count = self
            .content
            .as_ref()
            .map_or(0, |c| count_text_tokens(&c.join("")));
    }

    fn token_count(&self) -> usize {
        self.token_count
    }
}

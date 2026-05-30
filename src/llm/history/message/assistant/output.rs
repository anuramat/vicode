use serde::Deserialize;
use serde::Serialize;

use crate::llm::history::timing::Timing;
use crate::llm::history::timing::now;
use crate::llm::history::tokens::TokenCount;
use crate::llm::history::tokens::count_text_tokens;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct OutputItem {
    pub id: String,
    pub content: Vec<OutputContent>, // TODO when pushing, append to last item if the variant is the same

    pub token_count: usize,

    pub started_at: u64,
    pub ended_at: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum OutputContent {
    Text(String),
    Refusal(String),
}

impl TokenCount for OutputItem {
    fn recount(&mut self) {
        self.token_count = self
            .content
            .iter()
            .map(|c| match c {
                OutputContent::Text(t) | OutputContent::Refusal(t) => count_text_tokens(t),
            })
            .sum();
    }

    fn token_count(&self) -> usize {
        self.token_count
    }
}

impl OutputItem {
    pub fn new(id: String) -> Self {
        Self::new_at(id, now())
    }

    pub fn new_at(
        id: String,
        started_at: u64,
    ) -> Self {
        Self {
            id,
            started_at,
            ended_at: None,
            token_count: 0,
            content: Vec::new(),
        }
    }
}

impl Timing for OutputItem {
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

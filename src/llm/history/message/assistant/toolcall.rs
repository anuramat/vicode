use serde::Deserialize;
use serde::Serialize;

use crate::agent::tool::traits::ToolCallSerializable;
use crate::llm::history::timing::Timing;
use crate::llm::history::timing::now;
use crate::llm::history::timing::touch;
use crate::llm::history::tokens::TOOLCALL_OVERHEAD;
use crate::llm::history::tokens::TokenCount;
use crate::llm::history::tokens::count_text_tokens;

#[derive(Debug, Serialize, Clone, Deserialize)]
pub struct ToolCallItem {
    // TODO is this truly Option?
    pub id: Option<String>,
    pub call_id: String,
    #[serde(flatten)]
    pub task: Box<dyn ToolCallSerializable>,

    pub token_count: usize,

    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub ready_at: Option<u64>,
}

impl TokenCount for ToolCallItem {
    fn recount(&mut self) {
        self.token_count = TOOLCALL_OVERHEAD
            + count_text_tokens(&self.task.arguments())
            + self.task.output().map_or(0, |o| count_text_tokens(&o));
    }

    fn token_count(&self) -> usize {
        self.token_count
    }
}

impl ToolCallItem {
    pub const fn id(&self) -> &String {
        // HACK -- openai always has an actual id, but openrouter reuses call_id for id, and only sends it when creating the item;
        // regardless, it's a good enough heuristic -- we need *some* way to match calls and results;
        // I guess we could create a fake UUID on call creation, if this fails at some point?
        if let Some(id) = &self.id {
            id
        } else {
            &self.call_id
        }
    }

    pub fn touch_ready_at_now(&mut self) {
        touch(&mut self.ready_at, now());
    }
}

impl Timing for ToolCallItem {
    fn created_at(&self) -> u64 {
        self.started_at
    }

    fn started_at(&self) -> Option<u64> {
        Some(self.started_at)
    }

    fn ended_at(&self) -> Option<u64> {
        self.ended_at
    }

    fn ready_at(&self) -> Option<u64> {
        self.ready_at
    }
}

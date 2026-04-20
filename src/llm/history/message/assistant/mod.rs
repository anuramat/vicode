use ambassador::Delegate;
use anyhow::Result;
use anyhow::ensure;
use derive_more::From;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;

use crate::llm::history::tokens::TokenCount;
use crate::llm::history::tokens::ambassador_impl_TokenCount;

mod output;
mod reasoning;
mod toolcall;
pub use output::*;
pub use reasoning::*;
pub use toolcall::*;

use crate::llm::history::timing::Timing;
use crate::llm::history::timing::ambassador_impl_Timing;
use crate::llm::history::timing::now;
use crate::llm::history::timing::touch;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AssistantMessage {
    pub status: AssistantStatus,
    #[serde(with = "indexmap::map::serde_seq")]
    pub content: IndexMap<String, AssistantItem>,

    pub token_count: usize,

    pub created_at: u64,
    pub started_at: Option<u64>,
    pub ended_at: Option<u64>,
    pub ready_at: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub enum AssistantStatus {
    #[default]
    Queued,
    InProgress,
    Success,
    Error(String),
}

#[derive(Clone, Serialize, Deserialize, Debug, From, Delegate)]
#[delegate(TokenCount)]
#[delegate(Timing)]
pub enum AssistantItem {
    Output(OutputItem),
    Reasoning(ReasoningItem),
    ToolCall(ToolCallItem),
}

impl AssistantMessage {
    pub fn new(created_at: u64) -> Self {
        let mut result = Self {
            status: AssistantStatus::Queued,
            content: IndexMap::new(),
            token_count: 0,
            created_at,
            started_at: None,
            ended_at: None,
            ready_at: None,
        };
        result.recount();
        result
    }

    pub fn mark_started(
        &mut self,
        started_at: u64,
    ) -> Result<()> {
        self.started_at = Some(started_at);
        ensure!(
            matches!(self.status, AssistantStatus::Queued),
            "can't mark message as started, invalid current state: {:?}",
            self.status
        );
        self.status = AssistantStatus::InProgress;
        Ok(())
    }

    pub const fn touch_ended_at(
        &mut self,
        ms: u64,
    ) {
        touch(&mut self.ended_at, ms);
    }

    pub const fn touch_ready_at(
        &mut self,
        ms: u64,
    ) {
        touch(&mut self.ready_at, ms);
    }

    pub fn text_output(&self) -> String {
        self.content
            .values()
            .filter_map(|c| match c {
                AssistantItem::Output(m) => Some(&m.content),
                _ => None,
            })
            .flatten()
            .filter_map(|c| match c {
                OutputContent::Text(t) => Some(t.as_str()),
                OutputContent::Refusal(_) => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn recount_shallow(&mut self) {
        self.token_count = self
            .content
            .values()
            .map(TokenCount::token_count)
            .sum::<usize>();
    }
}

impl Timing for AssistantMessage {
    fn created_at(&self) -> u64 {
        self.created_at
    }

    fn started_at(&self) -> Option<u64> {
        self.started_at
    }

    fn ended_at(&self) -> Option<u64> {
        self.ended_at
    }

    fn ready_at(&self) -> Option<u64> {
        match (self.ended_at, self.ready_at) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
}

impl AssistantItem {
    pub fn id(&self) -> String {
        match self {
            Self::Output(msg) => &msg.id,
            Self::Reasoning(item) => &item.id,
            Self::ToolCall(tool) => tool.id(),
        }
        .clone()
    }

    const fn touch_ended_at(
        &mut self,
        ms: u64,
    ) {
        touch(self.ended_at_mut(), ms);
    }

    const fn ended_at_mut(&mut self) -> &mut Option<u64> {
        match self {
            Self::Output(item) => &mut item.ended_at,
            Self::Reasoning(item) => &mut item.ended_at,
            Self::ToolCall(item) => &mut item.ended_at,
        }
    }

    pub fn touch_ended_at_now(&mut self) -> u64 {
        let now = now();
        self.touch_ended_at(now);
        now
    }

    pub const fn set_started_at(
        &mut self,
        ms: u64,
    ) {
        match self {
            Self::Output(item) => item.started_at = ms,
            Self::Reasoning(item) => item.started_at = ms,
            Self::ToolCall(item) => item.started_at = ms,
        }
    }

    pub const fn set_ended_at(
        &mut self,
        ms: u64,
    ) {
        *self.ended_at_mut() = Some(ms);
    }
}

impl TokenCount for AssistantMessage {
    fn recount(&mut self) {
        self.content.values_mut().for_each(TokenCount::recount);
        self.recount_shallow();
    }

    fn token_count(&self) -> usize {
        self.token_count
    }
}

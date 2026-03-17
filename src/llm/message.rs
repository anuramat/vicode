use derive_more::From;
use derive_more::Into;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use strum::EnumTryAs;

use crate::agent::tool::traits::*;

#[derive(Clone, Serialize, Deserialize, Debug, From, EnumTryAs)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    Developer(DeveloperMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct DeveloperMessage {
    pub text: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct UserMessage {
    pub text: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, Into, From, Default)]
pub struct AssistantMessage {
    // TODO store start time and finish time/last update time maybe? idk
    pub finish_reason: AssistantMessageStatus,
    #[serde(with = "indexmap::map::serde_seq")]
    pub content: IndexMap<String, AssistantItem>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, EnumTryAs)]
pub enum AssistantMessageStatus {
    #[default]
    InProgress,
    Success,
    AbortedByUser,
    Error(String),
}

#[derive(Clone, Serialize, Deserialize, Debug, EnumTryAs, From)]
pub enum AssistantItem {
    Output(OutputItem),
    Reasoning(ReasoningItem),
    ToolCall(ToolCallItem),
}

// TODO rename "finished_at_ms" into "last_update_ms" or something; keep Option though

// finished_at_ms=None means that we didn't get any deltas after initializing the item, and so if
// we then get a new "item completed" event, we should use timestamp of that new event as the
// timestamp for the item

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct OutputItem {
    pub id: String,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub content: Vec<OutputContent>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum OutputContent {
    Text(String),
    Refusal(String),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ReasoningItem {
    pub id: String,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub content: Option<Vec<String>>,
    pub summary: Vec<String>,
    pub encrypted: Option<String>,
}

#[derive(Debug, Serialize, Clone, Deserialize)]
pub struct ToolCallItem {
    // TODO is this truly Option?
    pub id: Option<String>,
    pub call_id: String,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub executed_at_ms: Option<u64>,

    #[serde(flatten)]
    pub task: Box<dyn ToolCallSerializable>,
}

impl AssistantMessage {
    pub fn output(&self) -> String {
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
}

impl AssistantItem {
    pub fn id(&self) -> String {
        match self {
            AssistantItem::Output(msg) => &msg.id,
            AssistantItem::Reasoning(item) => &item.id,
            AssistantItem::ToolCall(tool) => tool.id(),
        }
        .clone()
    }

    pub fn get_start(&self) -> u64 {
        match self {
            AssistantItem::Output(item) => item.started_at_ms,
            AssistantItem::Reasoning(item) => item.started_at_ms,
            AssistantItem::ToolCall(item) => item.started_at_ms,
        }
    }

    pub fn set_start(
        &mut self,
        ms: u64,
    ) {
        match self {
            AssistantItem::Output(item) => item.started_at_ms = ms,
            AssistantItem::Reasoning(item) => item.started_at_ms = ms,
            AssistantItem::ToolCall(item) => item.started_at_ms = ms,
        }
    }

    pub fn get_finish(&self) -> Option<u64> {
        match self {
            AssistantItem::Output(item) => item.finished_at_ms,
            AssistantItem::Reasoning(item) => item.finished_at_ms,
            AssistantItem::ToolCall(item) => item.finished_at_ms,
        }
    }

    pub fn set_finish(
        &mut self,
        now: u64,
    ) {
        match self {
            AssistantItem::Output(item) => item.finished_at_ms = Some(now),
            AssistantItem::Reasoning(item) => item.finished_at_ms = Some(now),
            AssistantItem::ToolCall(item) => item.finished_at_ms = Some(now),
        }
    }

    pub fn finish(&mut self) {
        let now = now_ms();
        self.set_finish(now);
    }
}

impl ToolCallItem {
    pub fn id(&self) -> &String {
        // HACK -- openai always has an actual id, but openrouter reuses call_id for id, and only sends it when creating the item;
        // regardless, it's a good enough heuristic -- we need *some* way to match calls and results;
        // I guess we could create a fake UUID on call creation, if this fails at some point?
        if let Some(id) = &self.id {
            id
        } else {
            &self.call_id
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis()
        .try_into()
        .expect("timestamp overflow")
}

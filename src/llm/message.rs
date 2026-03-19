use std::fmt::Display;

use derive_more::From;
use derive_more::Into;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use strum::EnumTryAs;

use crate::agent::tool::traits::*;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct HistoryEntry {
    #[serde(default)]
    pub meta: MessageMeta,
    #[serde(flatten)]
    pub message: Message,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct MessageMeta {
    pub timing: ItemTiming,
    #[serde(default)]
    pub token_count: usize,
}

// TODO maybe drop default, instead use new()
// TODO maybe drop defaults and flatten above as well
impl Default for MessageMeta {
    fn default() -> Self {
        Self {
            timing: ItemTiming::new(),
            token_count: 0,
        }
    }
}

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

#[derive(Clone, Serialize, Default, Deserialize, Debug, Into, From)]
pub struct AssistantMessage {
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
pub struct ItemTiming {
    pub started_at_ms: u64,
    pub last_modified_ms: Option<u64>,
}

impl Display for ItemTiming {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self.last_modified_ms {
            None => f.write_str("tbd"),
            Some(end) => {
                let ms = end - self.started_at_ms;
                let s: f64 = (ms as f64) / 1000_f64;
                f.write_fmt(format_args!("{:.1}s", s))
            }
        }
    }
}

impl ItemTiming {
    pub fn new() -> Self {
        Self::with_start(now_ms())
    }

    pub fn with_start(started_at_ms: u64) -> Self {
        Self {
            started_at_ms,
            last_modified_ms: None,
        }
    }

    pub fn touch(&mut self) -> u64 {
        let now = now_ms();
        self.last_modified_ms = Some(now);
        now
    }

    pub fn touch_at(
        &mut self,
        at_ms: u64,
    ) {
        self.last_modified_ms = Some(at_ms);
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct OutputItem {
    pub id: String,
    pub timing: ItemTiming,
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
    pub timing: ItemTiming,
    pub content: Option<Vec<String>>,
    pub summary: Vec<String>,
    pub encrypted: Option<String>,
}

#[derive(Debug, Serialize, Clone, Deserialize)]
pub struct ToolCallItem {
    // TODO is this truly Option?
    pub id: Option<String>,
    pub call_id: String,
    pub timing: ItemTiming,
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
    pub fn timing(&self) -> &ItemTiming {
        match self {
            AssistantItem::Output(item) => &item.timing,
            AssistantItem::Reasoning(item) => &item.timing,
            AssistantItem::ToolCall(item) => &item.timing,
        }
    }

    pub fn timing_mut(&mut self) -> &mut ItemTiming {
        match self {
            AssistantItem::Output(item) => &mut item.timing,
            AssistantItem::Reasoning(item) => &mut item.timing,
            AssistantItem::ToolCall(item) => &mut item.timing,
        }
    }

    pub fn id(&self) -> String {
        match self {
            AssistantItem::Output(msg) => &msg.id,
            AssistantItem::Reasoning(item) => &item.id,
            AssistantItem::ToolCall(tool) => tool.id(),
        }
        .clone()
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

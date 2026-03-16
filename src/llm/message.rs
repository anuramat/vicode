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
    #[serde(with = "indexmap::map::serde_seq")]
    pub content: IndexMap<String, AssistantItem>,
}

#[derive(Clone, Serialize, Deserialize, Debug, EnumTryAs, From)]
pub enum AssistantItem {
    Output(OutputItem),
    Reasoning(ReasoningItem),
    ToolCall(ToolCallItem),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct OutputItem {
    pub id: String,
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
    pub content: Option<Vec<String>>,
    pub summary: Vec<String>,
    pub encrypted: Option<String>,
}

#[derive(Debug, Serialize, Clone, Deserialize)]
pub struct ToolCallItem {
    // TODO is this truly Option?
    pub id: Option<String>,
    pub call_id: String,

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

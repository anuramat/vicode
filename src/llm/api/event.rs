use crate::llm::delta::Delta;
use crate::llm::message::AssistantItem;

// TODO move to api/mod.rs
#[derive(Debug)]
pub enum StreamEvent {
    Delta(Delta),
    ItemDone(AssistantItem),
    ItemAdded(AssistantItem),
    Failed(String),
    Completed(Vec<AssistantItem>),
    Ignore,
}

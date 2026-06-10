use anyhow::Context;
use anyhow::Result;

use super::AssistantItem;
use super::AssistantMessage;
use super::HistoryState;
use super::OutputContent;
use super::OutputItem;
use super::ReasoningItem;
use super::timing::now;
use super::tokens::TokenCount;

#[derive(Debug, Clone)]
pub struct Delta {
    pub id: String,
    pub delta: DeltaContent,
    /// local arrival time of the api event, not one of the `Timing` slots;
    /// routed to the item's `started_at` on creation and `ended_at` on every delta
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub enum DeltaContent {
    Output(String),
    Reasoning(String),
    ReasoningSummary(String),
}

impl Delta {
    pub fn new(
        id: String,
        delta: DeltaContent,
    ) -> Self {
        Self::new_at(id, delta, now())
    }

    pub fn new_at(
        id: String,
        delta: DeltaContent,
        timestamp: u64,
    ) -> Self {
        Self {
            id,
            delta,
            timestamp,
        }
    }
}

impl AssistantMessage {
    fn push_reasoning_summary(
        &mut self,
        id: &str,
        delta: String,
        started_at: u64,
    ) -> Option<&mut AssistantItem> {
        let item = self
            .content
            .entry(id.to_string())
            .or_insert_with(|| ReasoningItem::new(id.to_string(), started_at).into());
        let AssistantItem::Reasoning(inner) = &mut *item else {
            return None;
        };
        inner.summary.push(delta);
        Some(item)
    }

    fn push_reasoning(
        &mut self,
        id: &str,
        delta: String,
        started_at: u64,
    ) -> Option<&mut AssistantItem> {
        let item = self
            .content
            .entry(id.to_string())
            .or_insert_with(|| ReasoningItem::new(id.to_string(), started_at).into());
        let AssistantItem::Reasoning(inner) = &mut *item else {
            return None;
        };
        inner.content.get_or_insert_default().push(delta);
        Some(item)
    }

    fn push_output(
        &mut self,
        id: &str,
        delta: String,
        started_at: u64,
    ) -> Option<&mut AssistantItem> {
        let item = self
            .content
            .entry(id.to_string())
            .or_insert_with(|| OutputItem::new(id.to_string(), started_at).into());
        let AssistantItem::Output(inner) = &mut *item else {
            return None;
        };
        inner.content.push(OutputContent::Text(delta));
        Some(item)
    }
}

impl HistoryState {
    pub fn push_delta(
        &mut self,
        item_delta: Delta,
    ) -> Result<()> {
        let msg = self
            .last_mut()
            .and_then(|v| v.try_as_assistant_mut())
            .context("last message is not an assistant message")?;
        let item = match item_delta.delta {
            DeltaContent::Output(delta) => {
                msg.push_output(&item_delta.id, delta, item_delta.timestamp)
            }
            DeltaContent::Reasoning(delta) => {
                msg.push_reasoning(&item_delta.id, delta, item_delta.timestamp)
            }
            DeltaContent::ReasoningSummary(delta) => {
                msg.push_reasoning_summary(&item_delta.id, delta, item_delta.timestamp)
            }
        }
        .context("delta type mismatch")?;
        item.touch_ended_at(item_delta.timestamp);
        item.recount();
        msg.recount_shallow();
        msg.touch_ended_at(item_delta.timestamp);
        self.recount_shallow();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::llm::history::message::Message;
    use crate::llm::history::timing::Timing;

    #[test]
    fn delta_does_not_rewind_item_ended_at() {
        let mut state = HistoryState::default();
        state.push(AssistantMessage::new(0).into());
        let mut item = OutputItem::new("out".into(), 1);
        item.ended_at = Some(10);
        state.push_item(AssistantItem::Output(item)).unwrap();
        state
            .push_delta(Delta {
                id: "out".into(),
                delta: DeltaContent::Output("hi".into()),
                timestamp: 5,
            })
            .unwrap();

        let Some(Message::Assistant(msg)) = state.last() else {
            panic!("expected assistant message");
        };
        assert_eq!(msg.content["out"].ended_at(), Some(10));
    }
}

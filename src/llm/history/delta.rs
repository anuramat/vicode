use anyhow::Context;
use anyhow::Result;

use super::AssistantItem;
use super::AssistantMessage;
use super::HistoryState;
use super::OutputContent;
use super::OutputItem;
use super::ReasoningItem;
use super::tokens::TokenCount;

#[derive(Debug, Clone)]
pub struct Delta {
    pub id: String,
    pub delta: DeltaContent,
}

#[derive(Debug, Clone)]
pub enum DeltaContent {
    Output(String),
    Reasoning(String),
    ReasoningSummary(String),
}

impl AssistantMessage {
    fn push_reasoning_summary(
        &mut self,
        id: &str,
        delta: String,
    ) -> Option<()> {
        let item = self
            .content
            .entry(id.to_string())
            .or_insert_with(|| ReasoningItem::new(id.to_string()).into());
        let AssistantItem::Reasoning(item) = item else {
            return None;
        };
        item.summary.push(delta);
        Some(())
    }

    fn push_reasoning(
        &mut self,
        id: &str,
        delta: String,
    ) -> Option<()> {
        let item = self
            .content
            .entry(id.to_string())
            .or_insert_with(|| ReasoningItem::new(id.to_string()).into());
        let AssistantItem::Reasoning(item) = item else {
            return None;
        };
        item.content.get_or_insert_default().push(delta);
        Some(())
    }

    fn push_output(
        &mut self,
        id: &str,
        delta: String,
    ) -> Option<()> {
        let item = self
            .content
            .entry(id.to_string())
            .or_insert_with(|| OutputItem::new(id.to_string()).into());
        let AssistantItem::Output(item) = item else {
            return None;
        };
        item.content.push(OutputContent::Text(delta));
        Some(())
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
        match item_delta.delta {
            DeltaContent::Output(delta) => msg.push_output(&item_delta.id, delta),
            DeltaContent::Reasoning(delta) => msg.push_reasoning(&item_delta.id, delta),
            DeltaContent::ReasoningSummary(delta) => {
                msg.push_reasoning_summary(&item_delta.id, delta)
            }
        }
        .context("delta type mismatch")?;
        let ended_at = {
            let item = msg
                .content
                .get_mut(&item_delta.id)
                .context("item not found")?;
            let touch_at = item.touch_ended_at_now();
            item.recount();
            touch_at
        };
        msg.recount_shallow();
        msg.touch_ended_at(ended_at);
        self.recount_shallow();
        Ok(())
    }
}

use crate::llm::history::History;
use crate::llm::message::AssistantMessage;
use crate::llm::message::Message;

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
        id: String,
        delta: String,
    ) -> Option<u64> {
        let item = self.content.get_mut(&id)?.try_as_reasoning_mut()?;
        let modified = item.timing.touch();
        item.summary.push(delta);
        Some(modified)
    }

    fn push_reasoning(
        &mut self,
        id: String,
        delta: String,
    ) -> Option<u64> {
        let reasoning = self.content.get_mut(&id)?.try_as_reasoning_mut()?;
        if reasoning.content.is_none() {
            reasoning.content = Some(Vec::new());
        }
        let item = reasoning.content.as_mut()?;
        let modified = reasoning.timing.touch();
        item.push(delta);
        Some(modified)
    }

    fn push_output(
        &mut self,
        id: String,
        delta: String,
    ) -> Option<u64> {
        let item = self.content.get_mut(&id)?.try_as_output_mut()?;
        item.content
            .push(crate::llm::message::OutputContent::Text(delta));
        Some(item.timing.touch())
    }
}

impl History {
    pub fn push_delta(
        &mut self,
        item_delta: Delta,
    ) {
        // XXX kinda ugly
        if let Some(modified) = if let Some(entry) = self.last()
            && let Message::Assistant(msg) = &mut entry.message
        {
            match item_delta.delta {
                DeltaContent::Output(delta) => msg.push_output(item_delta.id, delta),
                DeltaContent::Reasoning(delta) => msg.push_reasoning(item_delta.id, delta),
                DeltaContent::ReasoningSummary(delta) => {
                    msg.push_reasoning_summary(item_delta.id, delta)
                }
            }
        } else {
            None
        } {
            self.last().unwrap().meta.timing.touch_at(modified);
        }
    }
}

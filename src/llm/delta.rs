use crate::llm::history::History;
use crate::llm::message::AssistantMessage;
use crate::llm::message::Message;
use crate::llm::message::now_ms;

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
    ) -> Option<()> {
        let item = self.content.get_mut(&id)?.try_as_reasoning_mut()?;
        item.finished_at_ms = Some(now_ms());
        item.summary.push(delta);
        Some(())
    }

    fn push_reasoning(
        &mut self,
        id: String,
        delta: String,
    ) -> Option<()> {
        let reasoning = self.content.get_mut(&id)?.try_as_reasoning_mut()?;
        if reasoning.content.is_none() {
            reasoning.content = Some(Vec::new());
        }
        let item = reasoning.content.as_mut()?;
        reasoning.finished_at_ms = Some(now_ms());
        item.push(delta);
        Some(())
    }

    fn push_output(
        &mut self,
        id: String,
        delta: String,
    ) -> Option<()> {
        let item = self.content.get_mut(&id)?.try_as_output_mut()?;
        item.content
            .push(crate::llm::message::OutputContent::Text(delta));
        item.finished_at_ms = Some(now_ms());
        Some(())
    }
}

impl History {
    pub fn push_delta(
        &mut self,
        loc: usize,
        item_delta: Delta,
    ) {
        if let Some(Message::Assistant(msg)) = self.get_mut(loc) {
            match item_delta.delta {
                DeltaContent::Output(delta) => msg.push_output(item_delta.id, delta),
                DeltaContent::Reasoning(delta) => msg.push_reasoning(item_delta.id, delta),
                DeltaContent::ReasoningSummary(delta) => {
                    msg.push_reasoning_summary(item_delta.id, delta)
                }
            }
            .expect("failed to push delta");
        }
    }
}

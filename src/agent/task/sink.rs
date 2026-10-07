use anyhow::Result;
use tokio::sync::mpsc::UnboundedSender;

use crate::agent::event::AgentEvent;
use crate::agent::task::TaskId;
use crate::llm::history::AssistantEvent;

/// a task's line back to its agent: the turn's stream events, or a tool's
/// output chunks
#[derive(Clone, Debug)]
pub struct TaskSink {
    id: TaskId,
    tx: UnboundedSender<AgentEvent>,
}

impl TaskSink {
    pub fn new(
        id: TaskId,
        tx: UnboundedSender<AgentEvent>,
    ) -> Self {
        Self { id, tx }
    }

    pub fn stream(
        &self,
        event: AssistantEvent,
    ) -> Result<()> {
        self.tx.send(AgentEvent::Stream(self.id, event))?;
        Ok(())
    }

    pub fn output(
        &self,
        chunk: String,
    ) {
        drop(self.tx.send(AgentEvent::Output(self.id, chunk)));
    }
}

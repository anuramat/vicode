use anyhow::Result;
use tokio::sync::mpsc::Sender;

use crate::agent::handle::AgentEvent;
use crate::agent::task::ledger::TaskId;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;

#[derive(Clone)]
pub struct TaskHandle {
    tid: TaskId,
    generation: HistoryGeneration,
    tx: Sender<AgentEvent>,
}

#[derive(Clone)]
pub struct TurnHandle {
    pub task: TaskHandle,
    pub turn_type: TurnType,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum TurnType {
    Default,
    Compact,
}

impl TurnHandle {
    pub async fn send(
        &self,
        event: AssistantEvent,
    ) -> Result<()> {
        match self.turn_type {
            TurnType::Default => self.task.send(HistoryUpdate::TurnResponse(event)).await,
            TurnType::Compact => self.task.send(HistoryUpdate::CompactResponse(event)).await,
        }
    }
}

impl TaskHandle {
    pub fn new(
        tid: TaskId,
        generation: HistoryGeneration,
        tx: Sender<AgentEvent>,
    ) -> Self {
        Self {
            tid,
            generation,
            tx,
        }
    }

    pub async fn send(
        &self,
        event: HistoryUpdate,
    ) -> Result<()> {
        self.tx
            .send(AgentEvent::TaskEvent(self.tid, self.generation, event))
            .await?;
        Ok(())
    }
}

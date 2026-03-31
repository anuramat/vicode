use anyhow::Result;
use tokio::sync::mpsc::Sender;

use crate::agent::handle::AgentEvent;
use crate::agent::task::manager::TaskId;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::ResponseEvent;

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
pub enum TurnType {
    Default,
    Compact,
}

impl TurnHandle {
    pub async fn send(
        &self,
        event: ResponseEvent,
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
            .send(AgentEvent::TaskEvent(
                self.tid.clone(),
                self.generation,
                event,
            ))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

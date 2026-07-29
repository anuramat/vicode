use anyhow::Result;
use tokio::sync::mpsc::Sender;

use crate::agent::handle::AgentEvent;
use crate::agent::task::ledger::TaskId;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;

/// a tool's incremental output chunk, tagged with its call_id
pub type OutputChunk = (String, String);

/// per-call sink a tool streams incremental output through; chunks land in
/// the `Agent`'s accumulator + the app render stream, never in the core
#[derive(Clone, Debug)]
pub struct OutputSink {
    call_id: String,
    tx: Sender<OutputChunk>,
}

impl OutputSink {
    pub fn new(
        call_id: String,
        tx: Sender<OutputChunk>,
    ) -> Self {
        Self { call_id, tx }
    }

    /// backpressures on the `Agent` (bounded channel), never fails: a closed
    /// channel means the `Agent` is gone and the chunk has nowhere to go anyway
    pub async fn send(
        &self,
        chunk: String,
    ) {
        drop(self.tx.send((self.call_id.clone(), chunk)).await);
    }
}

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

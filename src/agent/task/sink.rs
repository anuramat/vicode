use anyhow::Result;
use tokio::sync::mpsc::Sender;

use crate::agent::handle::AgentEvent;
use crate::agent::task::ledger::TaskId;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;

/// tool output chunk, tagged with its `call_id`
pub type OutputChunk = (String, String);

/// for tool output
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

    pub async fn send(
        &self,
        chunk: String,
    ) {
        drop(self.tx.send((self.call_id.clone(), chunk)).await);
    }
}

#[derive(Clone)]
pub struct TurnHandle {
    tid: TaskId,
    generation: HistoryGeneration,
    turn_type: TurnType,
    tx: Sender<AgentEvent>,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum TurnType {
    Default,
    Compact,
}

impl TurnType {
    pub fn wrap(
        self,
        event: AssistantEvent,
    ) -> HistoryUpdate {
        match self {
            Self::Default => HistoryUpdate::TurnResponse(event),
            Self::Compact => HistoryUpdate::CompactResponse(event),
        }
    }
}

impl TurnHandle {
    pub fn new(
        tid: TaskId,
        generation: HistoryGeneration,
        turn_type: TurnType,
        tx: Sender<AgentEvent>,
    ) -> Self {
        Self {
            tid,
            generation,
            turn_type,
            tx,
        }
    }

    pub async fn send(
        &self,
        event: AssistantEvent,
    ) -> Result<()> {
        let event = self.turn_type.wrap(event);
        self.tx
            .send(AgentEvent::TaskEvent(self.tid, self.generation, event))
            .await?;
        Ok(())
    }
}

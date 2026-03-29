use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc::Sender;

use crate::agent::handle::AgentEvent;
use crate::agent::task::manager::TaskId;
use crate::llm::history::HistoryEvent;
use crate::llm::history::HistoryGeneration;

#[derive(Clone)]
pub struct TaskHandle {
    tid: TaskId,
    generation: HistoryGeneration,
    sink: Arc<dyn TaskSink>,
}

#[async_trait::async_trait]
trait TaskSink: Send + Sync {
    async fn send_history(
        &self,
        tid: TaskId,
        generation: HistoryGeneration,
        event: HistoryEvent,
    ) -> Result<()>;
}

struct AgentTaskSink {
    tx: Sender<AgentEvent>,
}

#[async_trait::async_trait]
impl TaskSink for AgentTaskSink {
    async fn send_history(
        &self,
        tid: TaskId,
        generation: HistoryGeneration,
        event: HistoryEvent,
    ) -> Result<()> {
        self.tx
            .send(AgentEvent::TaskEvent(tid, generation, event))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
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
            sink: Arc::new(AgentTaskSink { tx }),
        }
    }

    pub async fn history(
        &self,
        event: HistoryEvent,
    ) -> Result<()> {
        self.sink
            .send_history(self.tid.clone(), self.generation, event)
            .await
    }
}

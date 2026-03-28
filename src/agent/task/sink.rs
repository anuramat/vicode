use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc::Sender;

use crate::agent::AgentEvent;
use crate::agent::task::TaskDelta;
use crate::agent::task::TaskEvent;
use crate::agent::task::TaskId;
use crate::agent::task::TaskResult;

#[derive(Clone)]
pub struct TaskHandle {
    id: TaskId,
    sink: Arc<dyn TaskSink>,
}

#[async_trait::async_trait]
trait TaskSink: Send + Sync {
    async fn send(
        &self,
        id: TaskId,
        event: TaskEvent,
    ) -> Result<()>;
}

struct AgentTaskSink {
    tx: Sender<AgentEvent>,
}

#[async_trait::async_trait]
impl TaskSink for AgentTaskSink {
    async fn send(
        &self,
        id: TaskId,
        event: TaskEvent,
    ) -> Result<()> {
        self.tx
            .send(AgentEvent::Task(id.clone(), event))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

impl TaskHandle {
    pub fn new(
        id: TaskId,
        tx: Sender<AgentEvent>,
    ) -> Self {
        Self {
            id,
            sink: Arc::new(AgentTaskSink { tx }),
        }
    }

    pub async fn delta<T>(
        &self,
        delta: T,
    ) -> Result<()>
    where
        T: TaskDelta + 'static,
    {
        self.sink
            .send(self.id.clone(), TaskEvent::Delta(Box::new(delta)))
            .await
    }

    pub async fn result<T>(
        &self,
        result: T,
    ) -> Result<()>
    where
        T: TaskResult + 'static,
    {
        self.sink
            .send(self.id.clone(), TaskEvent::Result(Box::new(result)))
            .await
    }

    pub async fn error(
        &self,
        msg: String,
    ) -> Result<()> {
        self.sink.send(self.id.clone(), TaskEvent::Error(msg)).await
    }
}

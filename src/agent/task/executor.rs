use std::future::Future;

use anyhow::Result;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinSet;

use crate::agent::handle::AgentEvent;
use crate::agent::task::ledger::TaskId;
use crate::agent::task::sink::TaskHandle;
use crate::llm::history::HistoryGeneration;

/// runs tool calls, API requests, subagents on tokio tasks
#[derive(Debug, Default)]
pub struct TaskExecutor {
    tasks: JoinSet<()>,
}

impl TaskExecutor {
    pub fn spawn<F, Fut>(
        &mut self,
        tx: Sender<AgentEvent>,
        id: TaskId,
        generation: HistoryGeneration,
        task: F,
    ) where
        F: FnOnce(TaskHandle) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        while self.tasks.try_join_next().is_some() {}
        let handle = TaskHandle::new(id, generation, tx.clone());
        self.tasks.spawn(async move {
            let event = task(handle).await;
            tx.send(AgentEvent::TaskDone(id, event))
                .await
                .expect("failed to send task done event");
        });
    }

    pub async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
    }
}

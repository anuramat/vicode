use std::collections::HashMap;
use std::future::Future;

use anyhow::Result;
use tokio::sync::RwLock;
use tokio::sync::mpsc::Sender;
use tokio::task::AbortHandle;
use tokio::task::JoinSet;

use crate::agent::handle::AgentEvent;
use crate::agent::task::sink::*;
use crate::define_uuid;
use crate::llm::history::HistoryGeneration;

define_uuid!(TaskId);

/// manages tool calls, API requests, subagents
pub struct AgentTaskManager {
    tasks: JoinSet<()>,
    /// if a task is not in pending, we should ignore the results
    pending: HashMap<TaskId, AbortHandle>,
    /// lock to ensure that state/files are not modified while we're cloning the agent
    pub lock: RwLock<()>,
}

impl AgentTaskManager {
    pub fn new() -> Self {
        Self {
            tasks: JoinSet::new(),
            pending: HashMap::new(),
            lock: Default::default(),
        }
    }

    pub fn spawn<F, Fut>(
        &mut self,
        tx: Sender<AgentEvent>,
        generation: HistoryGeneration,
        task: F,
    ) where
        F: FnOnce(TaskHandle) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let id = TaskId::new();
        let handle = TaskHandle::new(id.clone(), generation, tx.clone());
        let task_id = id.clone();
        let wrapped = async move {
            let event: Result<()> = task(handle).await;
            tx.send(AgentEvent::TaskDone(task_id, event))
                .await
                .expect("failed to send task done event");
        };
        self.pending.insert(id, self.tasks.spawn(wrapped));
    }

    pub fn idle(&self) -> bool {
        self.pending.is_empty()
    }

    pub async fn abort(&mut self) {
        self.tasks.shutdown().await;
        self.pending.clear();
    }

    pub fn pending(
        &self,
        tid: &TaskId,
    ) -> bool {
        self.pending.contains_key(tid)
    }

    pub fn finish_task(
        &mut self,
        id: &TaskId,
    ) -> bool {
        if let Some(task) = self.pending.remove(id) {
            task.abort();
            return true;
        }
        false
    }
}

use std::collections::HashMap;
use std::future::Future;

use anyhow::Result;
use tokio::sync::RwLock;
use tokio::sync::mpsc::Sender;
use tokio::task::AbortHandle;
use tokio::task::JoinSet;

use crate::agent::AgentEvent;
use crate::agent::task::sink::*;
use crate::define_uuid;

define_uuid!(TaskId);

/// manages tool calls, API requests, subagents
pub struct AgentTaskManager {
    tasks: JoinSet<()>,
    /// if a task is not in pending, we should ignore the results
    pending: HashMap<TaskId, AbortHandle>,
    /// lock to ensure that state/files are not modified while we're cloning the agent
    pub lock: RwLock<()>,
}

impl Drop for AgentTaskManager {
    fn drop(&mut self) {
        self.tasks.abort_all();
    }
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
        task: F,
    ) where
        F: FnOnce(TaskHandle) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let id = TaskId::new();
        let handle = TaskHandle::new(id.clone(), tx);
        let wrapped = async move {
            let fallback = handle.clone();
            if let Err(e) = task(handle).await {
                fallback
                    .error(e.to_string())
                    .await
                    .expect("failed to send task error");
            }
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
    ) -> Result<()> {
        if let Some(task) = self.pending.remove(id) {
            task.abort();
            return Ok(());
        }
        panic!("task result applied but task not found in pending");
    }
}

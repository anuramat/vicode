use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::RwLock;
use tokio::sync::mpsc::Sender;
use tokio::task::AbortHandle;
use tokio::task::JoinSet;

use crate::agent::Agent;
use crate::agent::AgentEvent;
use crate::agent::replica::ReplicaResult;
use crate::define_uuid;
use crate::llm::history::HistoryEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::message::AssistantItem;
use crate::llm::message::ToolCallItem;

define_uuid!(TaskId);

#[derive(Debug)]
pub enum TaskResult {
    ToolCall(HistoryGeneration, ToolCallItem),
    AssistantResponse,
    ReplicaRun(ReplicaResult),
}

/// manages tool calls, API requests, subagents
#[derive(Default)]
pub struct AgentTaskManager {
    pub tasks: JoinSet<()>,
    /// if a task is not in pending, we should ignore the results
    pub pending: HashMap<TaskId, AbortHandle>,
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
        Self::default()
    }

    pub fn spawn<F>(
        &mut self,
        tx: Sender<AgentEvent>,
        fut: F,
    ) where
        F: Future<Output = TaskResult> + Send + 'static,
    {
        let id = TaskId::new();
        let wrapped = {
            let id = id.clone();
            async move {
                let output = fut.await;
                tx.send(AgentEvent::TaskDone(id, output))
                    .await
                    .expect("failed to send task result");
            }
        };
        self.pending.insert(id, self.tasks.spawn(wrapped));
    }

    pub async fn abort(&mut self) {
        self.tasks.shutdown().await;
        self.pending.clear();
    }
}

impl Agent {
    pub async fn apply_task_result(
        &mut self,
        id: TaskId,
        result: TaskResult,
    ) -> Result<()> {
        // NOTE it's important that we remove the task from pending only after applying the result
        if !self.tskmgr.pending.contains_key(&id) {
            // task is outdated, ignore
            return Ok(());
        }
        match result {
            TaskResult::ToolCall(generation, tool_call) => {
                self.handle_history(
                    generation,
                    HistoryEvent::ResponseItem(Box::new(AssistantItem::ToolCall(tool_call))),
                )
                .await?;
            }
            TaskResult::AssistantResponse => {
                // TODO why is this no op
            }
            TaskResult::ReplicaRun(run) => {
                self.handle_history(
                    self.state.context.history.generation(),
                    HistoryEvent::DeveloperMessage(run.report),
                )
                .await?;
            }
        }
        if self.tskmgr.pending.remove(&id).is_none() {
            // TODO should be impossible
            panic!("task result applied but task not found in pending");
        }
        Ok(())
    }
}

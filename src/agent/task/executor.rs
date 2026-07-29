use std::collections::HashMap;
use std::future::Future;

use anyhow::Result;
use tokio::task::JoinError;
use tokio::task::JoinSet;

use crate::agent::task::ledger::TaskId;
use crate::llm::history::message::ToolCallItem;

/// what a task's future hands the reaper on normal return
#[derive(Debug)]
pub enum TaskOutput {
    Turn(Result<()>),
    /// the resolved call item — for a streaming tool its text already went
    /// through the output sink, the item carries the terminal value
    Tool(Box<ToolCallItem>),
}

/// side map entry: identifies a task's ledger slot — and, for tools, the
/// history slot to finalize on panic/cancel (the item dies with the future)
#[derive(Debug)]
pub struct TaskMeta {
    pub id: TaskId,
    pub call_id: Option<String>,
}

/// runs tool calls and API requests on tokio tasks; `reap` is the single
/// resolver — every spawned task terminates exactly one reap, across
/// return, panic, and cancel alike
#[derive(Debug, Default)]
pub struct TaskExecutor {
    tasks: JoinSet<TaskOutput>,
    meta: HashMap<tokio::task::Id, TaskMeta>,
}

impl TaskExecutor {
    pub fn spawn_turn<F>(
        &mut self,
        id: TaskId,
        task: F,
    ) where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        let handle = self
            .tasks
            .spawn(async move { TaskOutput::Turn(task.await) });
        self.meta
            .insert(handle.id(), TaskMeta { id, call_id: None });
    }

    /// `call_id` is copied out before the item moves into the future
    pub fn spawn_tool<F>(
        &mut self,
        id: TaskId,
        call_id: String,
        task: F,
    ) where
        F: Future<Output = ToolCallItem> + Send + 'static,
    {
        let handle = self
            .tasks
            .spawn(async move { TaskOutput::Tool(Box::new(task.await)) });
        self.meta.insert(
            handle.id(),
            TaskMeta {
                id,
                call_id: Some(call_id),
            },
        );
    }

    /// harvest the next terminated task; None iff no tasks are in flight
    pub async fn reap(&mut self) -> Option<(TaskMeta, Result<TaskOutput, JoinError>)> {
        let (id, output) = match self.tasks.join_next_with_id().await? {
            Ok((id, output)) => (id, Ok(output)),
            Err(e) => (e.id(), Err(e)),
        };
        let meta = self.meta.remove(&id).expect("reaped an unregistered task");
        Some((meta, output))
    }

    pub fn abort_all(&mut self) {
        self.tasks.abort_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tid() -> TaskId {
        crate::agent::task::ledger::TaskLedger::default().register()
    }

    #[tokio::test]
    async fn returning_panicking_and_cancelled_tasks_each_reap_exactly_once() {
        let mut executor = TaskExecutor::default();
        executor.spawn_turn(tid(), async { Ok(()) });
        let (meta, output) = executor.reap().await.unwrap();
        assert_eq!(meta.call_id, None);
        assert!(matches!(output, Ok(TaskOutput::Turn(Ok(())))));

        executor.spawn_turn(tid(), async { panic!("boom") });
        let (_, output) = executor.reap().await.unwrap();
        assert!(output.unwrap_err().is_panic());

        executor.spawn_turn(tid(), std::future::pending());
        executor.abort_all();
        let (_, output) = executor.reap().await.unwrap();
        assert!(output.unwrap_err().is_cancelled());

        // each task terminated exactly one reap: the set is now empty
        assert!(executor.reap().await.is_none());
    }
}

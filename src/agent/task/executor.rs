use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use tokio::task::JoinSet;

use crate::agent::event::TaskResult;
use crate::agent::task::ledger::TaskId;

#[derive(Debug, Default)]
pub struct TaskExecutor {
    tasks: JoinSet<(TaskId, TaskResult)>,
}

impl TaskExecutor {
    /// run a task to its terminal; a panic becomes an `Err` naming the `kind`
    pub fn spawn(
        &mut self,
        id: TaskId,
        kind: &'static str,
        task: impl Future<Output = TaskResult> + Send + 'static,
    ) {
        self.tasks.spawn(async move {
            let result = AssertUnwindSafe(task)
                .catch_unwind()
                .await
                .unwrap_or_else(|panic| {
                    Err(format!("{kind} panicked: {}", panic_message(&*panic)))
                });
            (id, result)
        });
    }

    /// the next terminal; None iff no tasks are in flight. Cancelled tasks
    /// are skipped: only `abort_all` cancels, after the core resolved them
    pub async fn reap(&mut self) -> Option<(TaskId, TaskResult)> {
        loop {
            if let Ok(done) = self.tasks.join_next().await? {
                return Some(done);
            }
        }
    }

    pub fn abort_all(&mut self) {
        self.tasks.abort_all();
    }
}

pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::task::ledger::Task;
    use crate::agent::task::ledger::TaskLedger;

    #[tokio::test]
    async fn returning_and_panicking_tasks_reap_once_cancelled_ones_never() {
        let mut ledger = TaskLedger::default();
        let mut executor = TaskExecutor::default();

        let ok = ledger.register(Task::turn());
        executor.spawn(ok, "turn", async { Ok(None) });
        assert!(matches!(executor.reap().await, Some((id, Ok(None))) if id == ok));

        let boom = ledger.register(Task::turn());
        executor.spawn(boom, "tool", async { panic!("boom") });
        assert!(matches!(
            executor.reap().await,
            Some((id, Err(e))) if id == boom && e == "tool panicked: boom"
        ));

        executor.spawn(
            ledger.register(Task::turn()),
            "turn",
            std::future::pending(),
        );
        executor.abort_all();
        assert!(executor.reap().await.is_none());
    }
}

use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use tokio::task::JoinSet;

use crate::agent::event::TaskResult;
use crate::agent::task::ledger::TaskId;

#[derive(Debug)]
pub enum TaskExecutor {
    Live(JoinSet<(TaskId, TaskResult)>),
    /// records each task as (id, kind) and drops it unrun: the test feeds
    /// the task's events by hand
    #[cfg(test)]
    Held(Vec<(TaskId, &'static str)>),
}

impl Default for TaskExecutor {
    fn default() -> Self {
        Self::Live(JoinSet::new())
    }
}

impl TaskExecutor {
    /// run a task, converting panics to `Err`
    pub fn spawn(
        &mut self,
        id: TaskId,
        kind: &'static str,
        task: impl Future<Output = TaskResult> + Send + 'static,
    ) {
        match self {
            Self::Live(tasks) => {
                tasks.spawn(async move {
                    let result =
                        AssertUnwindSafe(task)
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|panic| {
                                Err(format!("{kind} panicked: {}", panic_message(&*panic)))
                            });
                    (id, result)
                });
            }
            #[cfg(test)]
            Self::Held(held) => held.push((id, kind)),
        }
    }

    pub async fn reap(&mut self) -> Option<(TaskId, TaskResult)> {
        match self {
            Self::Live(tasks) => loop {
                if let Ok(done) = tasks.join_next().await? {
                    return Some(done);
                }
            },
            #[cfg(test)]
            Self::Held(_) => None,
        }
    }

    pub fn abort_all(&mut self) {
        match self {
            Self::Live(tasks) => tasks.abort_all(),
            #[cfg(test)]
            Self::Held(_) => {}
        }
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
    use crate::agent::event::TaskOutput;
    use crate::agent::task::ledger::Task;
    use crate::agent::task::ledger::TaskLedger;

    #[tokio::test]
    async fn returning_and_panicking_tasks_reap_once_cancelled_ones_never() {
        let mut ledger = TaskLedger::default();
        let mut executor = TaskExecutor::default();

        let ok = ledger.register(Task::turn());
        executor.spawn(ok, "turn", async { Ok(TaskOutput::Turn) });
        assert!(matches!(executor.reap().await, Some((id, Ok(TaskOutput::Turn))) if id == ok));

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

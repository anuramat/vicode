//! the agent's in-flight tasks: what it knows about each, and the futures
//! that run them

pub mod sink;

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use tokio::task::JoinSet;

use crate::agent::event::TaskResult;
use crate::llm::history::HistoryGeneration;

/// loop-local task identifier
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct TaskId(u64);

/// what the agent knows about an in-flight task
#[derive(Debug)]
pub enum Task {
    Turn {
        generation: HistoryGeneration,
    },
    Tool {
        call_id: String,
        /// streamed output so far: the authoritative text of a streaming
        /// tool, and what an abort or a panic keeps
        partial: String,
    },
    /// a summary of the first `n_drop` messages, generated alongside the
    /// turns
    Compact {
        n_drop: usize,
    },
}

impl Task {
    fn kind(&self) -> &'static str {
        match self {
            Self::Turn { .. } => "turn",
            Self::Tool { .. } => "tool",
            Self::Compact { .. } => "summary",
        }
    }
}

/// in-flight tasks; a result whose task is not in here (aborted) is stale
#[derive(Debug, Default)]
pub struct Tasks {
    next: u64,
    by_id: BTreeMap<TaskId, Task>,
    executor: Executor,
}

#[derive(Debug)]
enum Executor {
    Live(JoinSet<(TaskId, TaskResult)>),
    /// records each task as (id, kind) and drops it unrun: the test feeds
    /// the task's events by hand
    #[cfg(test)]
    Held(Vec<(TaskId, &'static str)>),
}

impl Default for Executor {
    fn default() -> Self {
        Self::Live(JoinSet::new())
    }
}

impl Tasks {
    /// run `task` as the future `run` builds for its id, converting panics
    /// to `Err`
    pub fn spawn<F>(
        &mut self,
        task: Task,
        run: impl FnOnce(TaskId) -> F,
    ) -> TaskId
    where
        F: Future<Output = TaskResult> + Send + 'static,
    {
        let kind = task.kind();
        let id = self.insert(task);
        let future = run(id);
        match &mut self.executor {
            Executor::Live(set) => {
                set.spawn(async move {
                    let result = AssertUnwindSafe(future)
                        .catch_unwind()
                        .await
                        .unwrap_or_else(|panic| {
                            Err(format!("{kind} panicked: {}", panic_message(&*panic)))
                        });
                    (id, result)
                });
            }
            #[cfg(test)]
            Executor::Held(held) => held.push((id, kind)),
        }
        id
    }

    fn insert(
        &mut self,
        task: Task,
    ) -> TaskId {
        let id = TaskId(self.next);
        self.next += 1;
        self.by_id.insert(id, task);
        id
    }

    /// the next finished task's result, stale ones included
    pub async fn reap(&mut self) -> Option<(TaskId, TaskResult)> {
        match &mut self.executor {
            Executor::Live(set) => loop {
                if let Ok(done) = set.join_next().await? {
                    return Some(done);
                }
            },
            #[cfg(test)]
            Executor::Held(_) => None,
        }
    }

    pub fn get(
        &self,
        id: TaskId,
    ) -> Option<&Task> {
        self.by_id.get(&id)
    }

    pub fn get_mut(
        &mut self,
        id: TaskId,
    ) -> Option<&mut Task> {
        self.by_id.get_mut(&id)
    }

    pub fn finish(
        &mut self,
        id: TaskId,
    ) -> Option<Task> {
        self.by_id.remove(&id)
    }

    pub fn idle(&self) -> bool {
        self.by_id.is_empty()
    }

    /// a turn or one of its tools is in flight: the last message may still change
    pub fn in_turn(&self) -> bool {
        self.by_id
            .values()
            .any(|task| !matches!(task, Task::Compact { .. }))
    }

    pub fn compacting(&self) -> bool {
        self.by_id
            .values()
            .any(|task| matches!(task, Task::Compact { .. }))
    }

    /// forget every task and cancel its future: whatever a cancelled task
    /// still sends is stale
    pub fn abort_all(&mut self) -> Vec<Task> {
        match &mut self.executor {
            Executor::Live(set) => set.abort_all(),
            #[cfg(test)]
            Executor::Held(_) => {}
        }
        std::mem::take(&mut self.by_id).into_values().collect()
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
    use similar_asserts::assert_eq;

    use super::*;
    use crate::agent::event::TaskOutput;

    impl Task {
        /// a default turn at generation 0
        pub fn turn() -> Self {
            Self::Turn { generation: 0 }
        }
    }

    impl Tasks {
        /// tasks whose futures are recorded, never run
        pub fn held() -> Self {
            Self {
                executor: Executor::Held(Vec::new()),
                ..Self::default()
            }
        }

        /// the held tasks started since the last call, as (id, kind)
        pub fn take_held(&mut self) -> Vec<(TaskId, &'static str)> {
            let Executor::Held(held) = &mut self.executor else {
                unreachable!("tasks aren't held");
            };
            std::mem::take(held)
        }

        /// a task without a future: the test drives its events by hand
        pub fn register(
            &mut self,
            task: Task,
        ) -> TaskId {
            self.insert(task)
        }
    }

    #[test]
    fn register_finish_get_idle() {
        let mut tasks = Tasks::default();
        assert!(tasks.idle());

        let a = tasks.register(Task::turn());
        let b = tasks.register(Task::turn());
        assert!(!tasks.idle());
        assert!(tasks.get(a).is_some() && tasks.get(b).is_some());

        assert!(tasks.finish(a).is_some());
        assert!(tasks.finish(a).is_none());
        assert!(tasks.get(a).is_none());
        assert!(!tasks.idle());

        assert!(tasks.finish(b).is_some());
        assert!(tasks.idle());
    }

    #[test]
    fn compaction_is_neither_turn_work_nor_idle() {
        let mut tasks = Tasks::default();
        let compact = tasks.register(Task::Compact { n_drop: 1 });
        assert!(!tasks.idle() && !tasks.in_turn() && tasks.compacting());

        let turn = tasks.register(Task::turn());
        assert!(tasks.in_turn() && tasks.compacting());

        tasks.finish(compact);
        assert!(tasks.in_turn() && !tasks.compacting());
        tasks.finish(turn);
        assert!(tasks.idle() && !tasks.in_turn());
    }

    #[test]
    fn abort_all_keeps_next_so_ids_are_never_reused() {
        let mut tasks = Tasks::default();
        let a = tasks.register(Task::turn());
        assert_eq!(tasks.abort_all().len(), 1);
        assert!(tasks.idle());
        assert!(tasks.get(a).is_none());

        let b = tasks.register(Task::turn());
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn returning_and_panicking_tasks_reap_once_cancelled_ones_never() {
        let mut tasks = Tasks::default();

        let ok = tasks.spawn(Task::turn(), |_| async { Ok(TaskOutput::Turn) });
        assert!(matches!(tasks.reap().await, Some((id, Ok(TaskOutput::Turn))) if id == ok));

        let tool = Task::Tool {
            call_id: "call".into(),
            partial: String::new(),
        };
        let boom = tasks.spawn(tool, |_| async { panic!("boom") });
        assert!(matches!(
            tasks.reap().await,
            Some((id, Err(e))) if id == boom && e == "tool panicked: boom"
        ));

        tasks.spawn(Task::turn(), |_| std::future::pending());
        tasks.abort_all();
        assert!(tasks.reap().await.is_none());
    }

    #[test]
    fn held_tasks_record_their_kind() {
        let mut tasks = Tasks::held();
        let turn = tasks.spawn(Task::turn(), |_| std::future::pending());
        let summary = tasks.spawn(Task::Compact { n_drop: 1 }, |_| std::future::pending());
        assert_eq!(
            tasks.take_held(),
            vec![(turn, "turn"), (summary, "summary")]
        );
        assert!(tasks.take_held().is_empty());
    }
}

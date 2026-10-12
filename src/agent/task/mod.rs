pub mod sink;

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use futures::future::BoxFuture;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinSet;

use crate::agent::event::AgentEvent;
use crate::agent::event::TaskResult;
use crate::agent::task::sink::TaskSink;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct TaskId(u64);

#[derive(Debug)]
pub enum Task {
    Turn,
    Tool { call_id: String },
    Compact { n_drop: usize },
}

impl Task {
    fn kind(&self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Tool { .. } => "tool",
            Self::Compact { .. } => "summary",
        }
    }
}

#[derive(Debug)]
pub struct Tasks {
    next: u64,
    /// aborted tasks are dropped from this set, and the corresponding events should be ignored
    by_id: BTreeMap<TaskId, Task>,
    tx: UnboundedSender<AgentEvent>,
    executor: Box<dyn Executor>,
}

/// runs the task futures; dropping it cancels them
pub trait Executor: std::fmt::Debug + Send + Sync {
    fn spawn(
        &mut self,
        future: BoxFuture<'static, ()>,
    );

    fn abort_all(&mut self);
}

// `JoinSet::` names the inherent methods, `Self::` would read as recursion
#[allow(clippy::use_self)]
impl Executor for JoinSet<()> {
    fn spawn(
        &mut self,
        future: BoxFuture<'static, ()>,
    ) {
        // drop the handles of tasks that have finished since
        while self.try_join_next().is_some() {}
        JoinSet::spawn(self, future);
    }

    fn abort_all(&mut self) {
        JoinSet::abort_all(self);
    }
}

impl Tasks {
    pub fn new(tx: UnboundedSender<AgentEvent>) -> Self {
        Self {
            next: 0,
            by_id: BTreeMap::new(),
            tx,
            executor: Box::new(JoinSet::new()),
        }
    }

    /// run a task with panics converted to `Err`
    pub fn spawn<F>(
        &mut self,
        task: Task,
        run: impl FnOnce(TaskSink) -> F,
    ) -> TaskId
    where
        F: Future<Output = TaskResult> + Send + 'static,
    {
        let kind = task.kind();
        let id = self.insert(task);
        let sink = TaskSink::new(id, self.tx.clone());
        let future = run(sink.clone());
        self.executor.spawn(Box::pin(async move {
            let result = AssertUnwindSafe(future)
                .catch_unwind()
                .await
                .unwrap_or_else(|panic| {
                    Err(format!("{kind} panicked: {}", panic_message(&*panic)))
                });
            sink.done(result);
        }));
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

    /// a turn or one of its tools is in flight, thus the last message may still change
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

    pub fn abort_all(&mut self) {
        self.executor.abort_all();
        self.by_id.clear();
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
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::sync::oneshot;

    use super::*;
    use crate::agent::event::TaskOutput;

    /// drops every future unrun: the test feeds the task's events by hand
    #[derive(Debug)]
    struct Held;

    impl Executor for Held {
        fn spawn(
            &mut self,
            _: BoxFuture<'static, ()>,
        ) {
        }

        fn abort_all(&mut self) {}
    }

    impl Tasks {
        /// tasks whose futures are dropped unrun, so nothing is ever sent
        pub fn held() -> Self {
            Self {
                executor: Box::new(Held),
                ..Self::new(unbounded_channel().0)
            }
        }

        /// id of the next task to start
        pub fn next_id(&self) -> TaskId {
            TaskId(self.next)
        }

        /// tasks still live among those started from `from` on, as (id, kind)
        pub fn since(
            &self,
            from: TaskId,
        ) -> Vec<(TaskId, &'static str)> {
            self.by_id
                .range(from..)
                .map(|(&id, task)| (id, task.kind()))
                .collect()
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
        let mut tasks = Tasks::held();
        assert!(tasks.idle());

        let a = tasks.register(Task::Turn);
        let b = tasks.register(Task::Turn);
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
        let mut tasks = Tasks::held();
        let compact = tasks.register(Task::Compact { n_drop: 1 });
        assert!(!tasks.idle() && !tasks.in_turn() && tasks.compacting());

        let turn = tasks.register(Task::Turn);
        assert!(tasks.in_turn() && tasks.compacting());

        tasks.finish(compact);
        assert!(tasks.in_turn() && !tasks.compacting());
        tasks.finish(turn);
        assert!(tasks.idle() && !tasks.in_turn());
    }

    #[test]
    fn abort_all_keeps_next_so_ids_are_never_reused() {
        let mut tasks = Tasks::held();
        let a = tasks.register(Task::Turn);
        tasks.abort_all();
        assert!(tasks.idle());
        assert!(tasks.get(a).is_none());

        let b = tasks.register(Task::Turn);
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn tasks_send_done_after_their_events_cancelled_ones_never() {
        let (tx, mut rx) = unbounded_channel();
        let mut tasks = Tasks::new(tx);

        let ok = tasks.spawn(Task::Turn, |sink| async move {
            sink.output("chunk".into());
            Ok(TaskOutput::Turn)
        });
        assert!(
            matches!(rx.recv().await, Some(AgentEvent::Output(id, c)) if id == ok && c == "chunk")
        );
        assert!(
            matches!(rx.recv().await, Some(AgentEvent::Done(id, Ok(TaskOutput::Turn))) if id == ok)
        );

        let tool = Task::Tool {
            call_id: "call".into(),
        };
        let boom = tasks.spawn(tool, |_| async { panic!("boom") });
        assert!(matches!(
            rx.recv().await,
            Some(AgentEvent::Done(id, Err(e))) if id == boom && e == "tool panicked: boom"
        ));

        // the guard drops with the future: once it's gone, nothing can follow
        let (guard, cancelled) = oneshot::channel::<()>();
        tasks.spawn(Task::Turn, |_| async move {
            let _guard = guard;
            std::future::pending().await
        });
        tasks.abort_all();
        cancelled.await.unwrap_err();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn since_lists_live_tasks_from_the_given_id() {
        let mut tasks = Tasks::held();
        let first = tasks.spawn(Task::Turn, |_| std::future::pending());
        let from = tasks.next_id();
        let turn = tasks.spawn(Task::Turn, |_| std::future::pending());
        let summary = tasks.spawn(Task::Compact { n_drop: 1 }, |_| std::future::pending());
        assert_eq!(
            tasks.since(from),
            vec![(turn, "turn"), (summary, "summary")]
        );
        assert_eq!(tasks.since(first).len(), 3);

        tasks.finish(turn);
        assert_eq!(tasks.since(from), vec![(summary, "summary")]);
        assert!(tasks.since(tasks.next_id()).is_empty());
    }
}

use std::collections::BTreeMap;

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

/// in flight tasks; if a task is not in here, its results should be ignored
#[derive(Debug, Default)]
pub struct TaskLedger {
    next: u64,
    tasks: BTreeMap<TaskId, Task>,
}

impl TaskLedger {
    pub fn register(
        &mut self,
        task: Task,
    ) -> TaskId {
        let id = TaskId(self.next);
        self.next += 1;
        self.tasks.insert(id, task);
        id
    }

    pub fn get(
        &self,
        id: TaskId,
    ) -> Option<&Task> {
        self.tasks.get(&id)
    }

    pub fn get_mut(
        &mut self,
        id: TaskId,
    ) -> Option<&mut Task> {
        self.tasks.get_mut(&id)
    }

    pub fn finish(
        &mut self,
        id: TaskId,
    ) -> Option<Task> {
        self.tasks.remove(&id)
    }

    pub fn idle(&self) -> bool {
        self.tasks.is_empty()
    }

    /// a turn or one of its tools is in flight: the last message may still change
    pub fn in_turn(&self) -> bool {
        self.tasks
            .values()
            .any(|task| !matches!(task, Task::Compact { .. }))
    }

    pub fn compacting(&self) -> bool {
        self.tasks
            .values()
            .any(|task| matches!(task, Task::Compact { .. }))
    }

    pub fn clear(&mut self) -> Vec<Task> {
        std::mem::take(&mut self.tasks).into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Task {
        /// a default turn at generation 0
        pub fn turn() -> Self {
            Self::Turn { generation: 0 }
        }
    }

    #[test]
    fn register_finish_get_idle() {
        let mut ledger = TaskLedger::default();
        assert!(ledger.idle());

        let a = ledger.register(Task::turn());
        let b = ledger.register(Task::turn());
        assert!(!ledger.idle());
        assert!(ledger.get(a).is_some() && ledger.get(b).is_some());

        assert!(ledger.finish(a).is_some());
        assert!(ledger.finish(a).is_none());
        assert!(ledger.get(a).is_none());
        assert!(!ledger.idle());

        assert!(ledger.finish(b).is_some());
        assert!(ledger.idle());
    }

    #[test]
    fn compaction_is_neither_turn_work_nor_idle() {
        let mut ledger = TaskLedger::default();
        let compact = ledger.register(Task::Compact { n_drop: 1 });
        assert!(!ledger.idle() && !ledger.in_turn() && ledger.compacting());

        let turn = ledger.register(Task::turn());
        assert!(ledger.in_turn() && ledger.compacting());

        ledger.finish(compact);
        assert!(ledger.in_turn() && !ledger.compacting());
        ledger.finish(turn);
        assert!(ledger.idle() && !ledger.in_turn());
    }

    #[test]
    fn clear_keeps_next_so_ids_are_never_reused() {
        let mut ledger = TaskLedger::default();
        let a = ledger.register(Task::turn());
        assert_eq!(ledger.clear().len(), 1);
        assert!(ledger.idle());
        assert!(ledger.get(a).is_none());

        let b = ledger.register(Task::turn());
        assert_ne!(a, b);
    }
}

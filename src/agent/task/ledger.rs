use std::collections::BTreeMap;

use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;

/// loop-local task identifier
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct TaskId(u64);

#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum TurnType {
    Default,
    Compact,
}

impl TurnType {
    pub fn wrap(
        self,
        event: AssistantEvent,
    ) -> HistoryUpdate {
        match self {
            Self::Default => HistoryUpdate::TurnResponse(event),
            Self::Compact => HistoryUpdate::CompactResponse(event),
        }
    }
}

/// what the core knows about an in-flight task
#[derive(Debug)]
pub enum Task {
    Turn {
        generation: HistoryGeneration,
        turn_type: TurnType,
    },
    Tool {
        call_id: String,
        /// streamed output so far: the authoritative text of a streaming
        /// tool, and what an abort or a panic keeps
        partial: String,
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

    /// forget every task, in registration order
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
            Self::Turn {
                generation: 0,
                turn_type: TurnType::Default,
            }
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

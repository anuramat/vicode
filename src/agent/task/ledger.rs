use std::collections::HashSet;

/// loop-local task identifier: never persisted, never crosses agents
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct TaskId(u64);

/// pure bookkeeping of in-flight tasks; if a task is not in pending, its
/// results should be ignored
#[derive(Debug, Default)]
pub struct TaskLedger {
    next: u64,
    pending: HashSet<TaskId>,
}

impl TaskLedger {
    pub fn register(&mut self) -> TaskId {
        let id = TaskId(self.next);
        self.next += 1;
        self.pending.insert(id);
        id
    }

    pub fn finish(
        &mut self,
        id: &TaskId,
    ) -> bool {
        self.pending.remove(id)
    }

    pub fn pending(
        &self,
        id: &TaskId,
    ) -> bool {
        self.pending.contains(id)
    }

    pub fn idle(&self) -> bool {
        self.pending.is_empty()
    }

    /// keeps `next`: id reuse could alias a queued stale TaskEvent to a new task
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_finish_pending_idle() {
        let mut ledger = TaskLedger::default();
        assert!(ledger.idle());

        let a = ledger.register();
        let b = ledger.register();
        assert!(!ledger.idle());
        assert!(ledger.pending(&a) && ledger.pending(&b));

        assert!(ledger.finish(&a));
        assert!(!ledger.finish(&a));
        assert!(!ledger.pending(&a));
        assert!(!ledger.idle());

        assert!(ledger.finish(&b));
        assert!(ledger.idle());
    }

    #[test]
    fn clear_keeps_next_so_ids_are_never_reused() {
        let mut ledger = TaskLedger::default();
        let a = ledger.register();
        ledger.clear();
        assert!(ledger.idle());
        assert!(!ledger.pending(&a));

        let b = ledger.register();
        assert_ne!(a, b);
    }
}

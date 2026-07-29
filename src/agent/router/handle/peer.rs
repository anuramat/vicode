//! inter-agent commands (`send`/`inspect`/`wait`/`list`): every target is
//! root-checked, so a tab can never reach into another

use std::collections::HashSet;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::oneshot;

use super::inbound;
use crate::agent::AgentId;
use crate::agent::router::AgentRouter;
use crate::agent::router::Waiter;
use crate::agent::router::api::ListEntry;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::WaitResult;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::NodeStatus;

impl AgentRouter {
    pub fn handle_send(
        &mut self,
        caller: AgentId,
        target: AgentId,
        text: String,
    ) -> Result<(), RouterError> {
        if self.same_tab(&caller, &target).is_none() {
            return Err(RouterError::Unreachable);
        }
        let msg = inbound(&caller, &text);
        let node = self.graph.get_mut(&target).expect("checked above");
        if node.status == NodeStatus::Dead {
            return Err(RouterError::Unreachable);
        }
        let Some(tx) = node.mailbox.clone() else {
            return Err(RouterError::Unreachable);
        };
        match tx.try_send(msg) {
            Ok(()) => {
                // the delivery commits the target to a turn: `delivered`
                // outruns `processed`, so a racing `wait` registers instead
                // of firing stale (M2)
                node.delivered += 1;
                if node.status == NodeStatus::Idle {
                    node.status = NodeStatus::Running;
                }
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(RouterError::Busy),
            Err(TrySendError::Closed(_)) => {
                self.fail_runtime(&target, "agent runtime mailbox closed".into(), true);
                Err(RouterError::Unreachable)
            }
        }
    }

    pub fn handle_inspect(
        &mut self,
        caller: &AgentId,
        target: &AgentId,
    ) -> Result<NodeStatus, RouterError> {
        self.same_tab(caller, target)
            .map(|n| n.status)
            .ok_or(RouterError::Unreachable)
    }

    pub fn handle_wait(
        &mut self,
        caller: AgentId,
        target: AgentId,
        done: oneshot::Sender<Result<WaitResult, RouterError>>,
    ) {
        let Some(result) = self.same_tab(&caller, &target).map(AgentNode::wait_result) else {
            drop(done.send(Err(RouterError::Unreachable)));
            return;
        };
        match result.status {
            NodeStatus::Idle | NodeStatus::Dead => drop(done.send(Ok(result))),
            // Spawning/Running: a delivered message committed the target to a turn.
            _ => {
                if self.would_deadlock(&caller, &target) {
                    drop(done.send(Err(RouterError::WouldDeadlock)));
                } else {
                    self.waiters
                        .entry(target)
                        .or_default()
                        .push(Waiter { caller, done });
                }
            }
        }
    }

    /// would a `caller → target` edge close a cycle in the live wait-for
    /// graph? closed registrations are pruned lazily (skipped)
    fn would_deadlock(
        &self,
        caller: &AgentId,
        target: &AgentId,
    ) -> bool {
        let mut stack = vec![target];
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            if id == caller {
                return true;
            }
            if !seen.insert(id) {
                continue;
            }
            for (waited_on, waiters) in &self.waiters {
                if waiters
                    .iter()
                    .any(|w| !w.done.is_closed() && &w.caller == id)
                {
                    stack.push(waited_on);
                }
            }
        }
        false
    }

    pub fn handle_list(
        &self,
        caller: &AgentId,
        subtree: bool,
    ) -> Option<Vec<ListEntry>> {
        let root = &self.graph.get(caller)?.root;
        let mut members: Vec<ListEntry> = self
            .graph
            .iter()
            .filter(|(_, n)| &n.root == root)
            .filter(|(id, _)| !subtree || self.descends(id, caller))
            .map(|(id, n)| ListEntry {
                id: id.clone(),
                parent: n.parent.clone(),
                status: n.status,
            })
            .collect();
        members.sort_by(|a, b| a.id.cmp(&b.id));
        Some(members)
    }
}

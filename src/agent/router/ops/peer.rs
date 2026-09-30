//! inter-agent operations (`send`/`inspect`/`wait`/`list`): every target is
//! root-checked, so a tab can never reach into another

use std::collections::HashSet;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::oneshot;

use super::inbound;
use crate::agent::AgentId;
use crate::agent::router::Router;
use crate::agent::router::RouterState;
use crate::agent::router::Waiter;
use crate::agent::router::api::ListEntry;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::WaitResult;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::NodeStatus;

impl Router {
    pub fn send_message(
        &self,
        caller: &AgentId,
        target: &AgentId,
        text: &str,
    ) -> Result<(), RouterError> {
        let s = &mut *self.lock();
        if s.same_tab(caller, target).is_none() {
            return Err(RouterError::Unreachable);
        }
        let node = s.graph.get_mut(target).expect("checked above");
        if node.status == NodeStatus::Dead {
            return Err(RouterError::Unreachable);
        }
        let Some(tx) = node.mailbox.clone() else {
            return Err(RouterError::Unreachable);
        };
        match tx.try_send(inbound(caller, text)) {
            Ok(()) => {
                // the delivery commits the target to a turn: `delivered`
                // outruns `processed`, so a racing `wait` registers instead
                // of firing stale
                node.delivered += 1;
                if node.status == NodeStatus::Idle {
                    node.status = NodeStatus::Running;
                }
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(RouterError::Busy),
            Err(TrySendError::Closed(_)) => {
                s.fail_runtime(target, "agent runtime mailbox closed".into(), true);
                Err(RouterError::Unreachable)
            }
        }
    }

    pub fn inspect(
        &self,
        caller: &AgentId,
        target: &AgentId,
    ) -> Result<NodeStatus, RouterError> {
        self.lock()
            .same_tab(caller, target)
            .map(|n| n.status)
            .ok_or(RouterError::Unreachable)
    }

    /// suspends until the target next goes idle (or dies)
    pub async fn wait(
        &self,
        caller: &AgentId,
        target: &AgentId,
    ) -> Result<WaitResult, RouterError> {
        let rx = {
            let s = &mut *self.lock();
            let Some(result) = s.same_tab(caller, target).map(AgentNode::wait_result) else {
                return Err(RouterError::Unreachable);
            };
            match result.status {
                NodeStatus::Idle | NodeStatus::Dead => return Ok(result),
                // Spawning/Running: a delivered message committed the target to a turn.
                _ if s.would_deadlock(caller, target) => return Err(RouterError::WouldDeadlock),
                _ => s.register_waiter(caller, target),
            }
        };
        // a waiter is only ever dropped unfired with its target
        rx.await.unwrap_or(Err(RouterError::Unreachable))
    }

    /// `None` = unknown caller
    pub fn list(
        &self,
        caller: &AgentId,
        subtree: bool,
    ) -> Option<Vec<ListEntry>> {
        let s = &*self.lock();
        let root = &s.graph.get(caller)?.root;
        let mut members: Vec<ListEntry> = s
            .graph
            .iter()
            .filter(|(_, n)| &n.root == root)
            .filter(|(id, _)| !subtree || s.descends(id, caller))
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

impl RouterState {
    pub fn register_waiter(
        &mut self,
        caller: &AgentId,
        target: &AgentId,
    ) -> oneshot::Receiver<Result<WaitResult, RouterError>> {
        let (done, rx) = oneshot::channel();
        self.waiters
            .entry(target.clone())
            .or_default()
            .push(Waiter {
                caller: caller.clone(),
                done,
            });
        rx
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
}

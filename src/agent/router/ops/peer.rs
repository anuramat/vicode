//! inter-agent operations (`send`/`inspect`/`wait`/`list`): every target is
//! root-checked, so a tab can never reach into another

use std::collections::HashSet;

use tokio::sync::oneshot;

use super::peer_message;
use crate::agent::AgentId;
use crate::agent::event::Mail;
use crate::agent::router::Router;
use crate::agent::router::RouterState;
use crate::agent::router::Waiter;
use crate::agent::router::api::ListEntry;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::TurnOutcome;
use crate::agent::router::api::WaitId;
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
        s.same_tab(caller, target).ok_or(RouterError::Unreachable)?;
        s.deliver(target, peer_message(caller, text))
    }

    pub fn inspect(
        &self,
        caller: &AgentId,
        target: &AgentId,
    ) -> Result<NodeStatus, RouterError> {
        self.lock()
            .same_tab(caller, target)
            .map(AgentNode::status)
            .ok_or(RouterError::Unreachable)
    }

    /// suspends until the target has handled everything delivered to it
    /// before this wait, and is idle (or dies)
    pub async fn wait(
        &self,
        caller: &AgentId,
        target: &AgentId,
    ) -> Result<WaitResult, RouterError> {
        let rx = {
            let s = &mut *self.lock();
            let node = s.same_tab(caller, target).ok_or(RouterError::Unreachable)?;
            if let Some(death) = node.death() {
                return Ok(death);
            }
            if s.would_deadlock(caller, target) {
                return Err(RouterError::WouldDeadlock);
            }
            s.enqueue_wait(caller, target)
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
                status: n.status(),
            })
            .collect();
        members.sort_by(|a, b| a.id.cmp(&b.id));
        Some(members)
    }

    /// the agent got to its delivered `waits` idle: fire exactly those
    pub fn settle(
        &self,
        aid: &AgentId,
        waits: &[WaitId],
        outcome: &TurnOutcome,
    ) {
        let result = WaitResult {
            status: NodeStatus::Idle,
            outcome: outcome.clone(),
        };
        self.lock()
            .fire_waiters(aid, |w| waits.contains(&w.id), &Ok(result));
    }
}

impl RouterState {
    /// register a waiter and deliver its marker behind everything already in
    /// the target's mailbox; a closed mailbox fires it `Dead` right away
    pub fn enqueue_wait(
        &mut self,
        caller: &AgentId,
        target: &AgentId,
    ) -> oneshot::Receiver<Result<WaitResult, RouterError>> {
        let id = WaitId(self.next_wait);
        self.next_wait += 1;
        let (done, rx) = oneshot::channel();
        self.waiters
            .entry(target.clone())
            .or_default()
            .push(Waiter {
                id,
                caller: caller.clone(),
                done,
            });
        drop(self.deliver(target, Mail::Wait(id)));
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

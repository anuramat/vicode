//! the router's operations, one submodule per concern; helpers shared
//! across them live here

use std::collections::BTreeSet;

use super::RouterState;
use super::Waiter;
use crate::agent::AgentId;
use crate::agent::event::Mail;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::WaitResult;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::Runtime;
use crate::llm::history::message::UserMessage;
use crate::utils::now;

mod archive;
mod peer;
pub mod runtime;
mod spawn;

impl RouterState {
    /// A runtime failure is terminal for this process. The durable live
    /// graph record remains, so a full application restart retries it.
    pub fn fail_runtime(
        &mut self,
        aid: &AgentId,
        error: String,
        abort: bool,
    ) {
        let Some(node) = self.graph.get_mut(aid) else {
            return;
        };
        if matches!(node.runtime, Runtime::Dead(_)) {
            return;
        }
        if let Runtime::Live { abort: handle, .. } =
            std::mem::replace(&mut node.runtime, Runtime::Dead(error))
            && abort
        {
            handle.abort();
        }
        let death = node.death().expect("just died");
        self.fire_waiters(aid, |_| true, &Ok(death));
    }

    /// the one delivery path: FIFO into the target's mailbox, which exists
    /// from node creation; a closed one means the runtime is gone
    pub fn deliver(
        &mut self,
        aid: &AgentId,
        mail: Mail,
    ) -> Result<(), RouterError> {
        let node = self.graph.get(aid).ok_or(RouterError::Unreachable)?;
        if matches!(node.runtime, Runtime::Dead(_)) {
            return Err(RouterError::Unreachable);
        }
        if node.mailbox.send(mail).is_err() {
            self.fail_runtime(aid, "agent runtime mailbox closed".into(), true);
            return Err(RouterError::Unreachable);
        }
        Ok(())
    }

    /// resolve `aid`'s waiters matching `pick` with `outcome`
    pub fn fire_waiters(
        &mut self,
        aid: &AgentId,
        pick: impl Fn(&Waiter) -> bool,
        outcome: &Result<WaitResult, RouterError>,
    ) {
        let Some(waiters) = self.waiters.remove(aid) else {
            return;
        };
        let (fired, kept): (Vec<_>, Vec<_>) = waiters.into_iter().partition(|w| pick(w));
        for waiter in fired {
            drop(waiter.done.send(outcome.clone()));
        }
        if !kept.is_empty() {
            self.waiters.insert(aid.clone(), kept);
        }
    }

    /// root check: the target's node iff the caller exists and shares its root
    fn same_tab(
        &self,
        caller: &AgentId,
        target: &AgentId,
    ) -> Option<&AgentNode> {
        let root = &self.graph.get(caller)?.root;
        self.graph.get(target).filter(|n| &n.root == root)
    }

    /// id == ancestor, or ancestor is reachable by walking parent edges up
    fn descends(
        &self,
        id: &AgentId,
        ancestor: &AgentId,
    ) -> bool {
        let mut cur = id;
        loop {
            if cur == ancestor {
                return true;
            }
            match self.graph.get(cur).and_then(|n| n.parent.as_ref()) {
                Some(parent) => cur = parent,
                None => return false,
            }
        }
    }
}

/// inter-agent message: user-role, tagged in-body with the sender id —
/// stamped by the router, so it can't be spoofed
fn peer_message(
    sender: &AgentId,
    text: &str,
) -> Mail {
    Mail::Message(UserMessage::new(format!("[from: {sender}]\n{text}"), now()))
}

/// smallest free variant per name — `b`, `b-2`, `b-3`, … — so allocation is
/// collision-free by construction and never reuses an archived id
pub fn free_variant(
    base: &str,
    taken: &BTreeSet<AgentId>,
) -> AgentId {
    (1..)
        .map(|n| {
            AgentId::from(if n == 1 {
                base.to_string()
            } else {
                format!("{base}-{n}")
            })
        })
        .find(|c| !taken.contains(c))
        .expect("some variant is free")
}

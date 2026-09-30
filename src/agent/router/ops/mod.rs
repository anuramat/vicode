//! the router's operations, one submodule per concern; helpers shared
//! across them live here

use std::collections::BTreeSet;

use super::RouterState;
use crate::agent::AgentId;
use crate::agent::event::AgentEvent;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::WaitResult;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::NodeStatus;
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
        if node.status == NodeStatus::Dead {
            return;
        }
        if let Some(handle) = node.abort.take()
            && abort
        {
            handle.abort();
        }
        node.mailbox = None;
        node.user_tx = None;
        node.status = NodeStatus::Dead;
        node.outcome.error = Some(error);
        let result = node.wait_result();
        self.fire_waiters(aid, Ok(result));
    }

    fn fire_waiters(
        &mut self,
        aid: &AgentId,
        outcome: Result<WaitResult, RouterError>,
    ) {
        for waiter in self.waiters.remove(aid).unwrap_or_default() {
            drop(waiter.done.send(outcome.clone()));
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

/// inbound inter-agent message: user-role, tagged in-body with the sender id
/// — stamped by the router, so it can't be spoofed
fn inbound(
    sender: &AgentId,
    text: &str,
) -> AgentEvent {
    AgentEvent::Inbound(UserMessage::new(format!("[from: {sender}]\n{text}"), now()))
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

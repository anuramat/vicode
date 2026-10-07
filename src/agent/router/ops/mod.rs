//! the router's operations, one submodule per concern; helpers shared
//! across them live here

use std::collections::BTreeSet;

use super::RouterState;
use crate::agent::AgentId;
use crate::agent::router::graph::AgentNode;
use crate::llm::history::message::PeerMessage;
use crate::utils::now;

mod archive;
mod peer;
pub mod runtime;
mod spawn;

impl RouterState {
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

/// inter-agent message: developer-role, tagged in-body with the sender id —
/// stamped by the router, so it can't be spoofed
fn peer_message(
    sender: &AgentId,
    text: &str,
) -> PeerMessage {
    PeerMessage::new(sender, text, now())
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

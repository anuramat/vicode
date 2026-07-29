//! inter-agent operations (`send`/`list`): every target is root-checked,
//! so a tab can never reach into another

use super::peer_message;
use crate::agent::AgentId;
use crate::agent::router::Router;
use crate::agent::router::api::ListEntry;
use crate::agent::router::api::RouterError;
use crate::agent::router::graph::Runtime;

impl Router {
    pub fn send_message(
        &self,
        caller: &AgentId,
        target: &AgentId,
        text: &str,
    ) -> Result<(), RouterError> {
        let s = &*self.lock();
        let node = s.same_tab(caller, target).ok_or(RouterError::Unreachable)?;
        if let Runtime::Dead(error) = &node.runtime {
            return Err(RouterError::Dead(error.clone()));
        }
        // the mailbox exists from node creation; it's closed only once the
        // runtime is gone, whose supervisor records the actual death reason
        node.mailbox
            .send(peer_message(caller, text))
            .map_err(|_| RouterError::Dead("agent runtime mailbox closed".into()))
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
}

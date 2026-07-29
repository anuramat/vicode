//! the router's command handlers: `dispatch` fans each [`RouterCommand`] out
//! to a handler in a submodule; helpers shared across them live here

use std::collections::BTreeSet;

use anyhow::Result;

use super::AgentRouter;
use super::RouterCommand;
use crate::agent::AgentId;
use crate::agent::handle::AgentEvent;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::WaitResult;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::NodeStatus;
use crate::llm::history::message::UserMessage;
use crate::utils::now;

mod archive;
mod peer;
mod runtime;
mod spawn;

impl AgentRouter {
    pub fn dispatch(
        &mut self,
        cmd: RouterCommand,
    ) {
        match cmd {
            RouterCommand::RegisterRoot { aid, done } => {
                drop(done.send(self.handle_register_root(aid)));
            }
            RouterCommand::Forward { aid, event, done } => {
                drop(done.send(self.handle_forward(aid, event)));
            }
            RouterCommand::Allocate { done } => drop(done.send(self.allocate())),
            #[cfg(test)]
            RouterCommand::Shutdown { aid, done } => self.handle_shutdown(&aid, done),
            RouterCommand::Status { aid, ping } => self.handle_status(aid, ping),
            RouterCommand::AttachRuntime { aid, runtime, done } => {
                drop(done.send(self.handle_attach_runtime(aid, runtime)));
            }
            RouterCommand::RuntimeDown { aid, error } => self.handle_runtime_down(&aid, error),
            RouterCommand::Spawn {
                parent,
                capture,
                prompt,
                done,
            } => self.handle_spawn(parent, capture, prompt, done),
            RouterCommand::Send {
                caller,
                target,
                text,
                done,
            } => drop(done.send(self.handle_send(caller, target, text))),
            RouterCommand::Inspect {
                caller,
                target,
                done,
            } => drop(done.send(self.handle_inspect(&caller, &target))),
            RouterCommand::Wait {
                caller,
                target,
                done,
            } => self.handle_wait(caller, target, done),
            RouterCommand::List {
                caller,
                subtree,
                done,
            } => drop(done.send(self.handle_list(&caller, subtree))),
            RouterCommand::Archive {
                caller,
                target,
                done,
            } => self.handle_archive(caller, target, done),
            RouterCommand::ArchiveTab { primary, done } => self.handle_archive_tab(primary, done),
            RouterCommand::RollbackSpawn { aid, done } => self.handle_rollback_spawn(aid, done),
            #[cfg(test)]
            RouterCommand::WaitIdle { aid, done } => self.handle_wait_idle(aid, done),
        }
    }

    /// A runtime failure is terminal for this process. The durable live
    /// graph record remains, so a full application restart retries it.
    fn fail_runtime(
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
/// collision-free by construction and never reuses an archived id (H2)
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

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::*;
    use crate::agent::router::Waiter;

    impl AgentRouter {
        pub fn handle_shutdown(
            &mut self,
            aid: &AgentId,
            done: oneshot::Sender<Result<()>>,
        ) {
            let result = if let Some(abort) = self.graph.get(aid).and_then(|n| n.abort.clone()) {
                abort.abort();
                self.fail_runtime(aid, "agent runtime cancelled by test".into(), false);
                Ok(())
            } else {
                Err(anyhow::anyhow!("no live runtime for {aid}"))
            };
            drop(done.send(result));
        }

        pub fn handle_wait_idle(
            &mut self,
            aid: AgentId,
            done: oneshot::Sender<Result<WaitResult, RouterError>>,
        ) {
            match self.graph.get(&aid) {
                None => drop(done.send(Err(RouterError::Unreachable))),
                Some(node) if matches!(node.status, NodeStatus::Idle | NodeStatus::Dead) => {
                    drop(done.send(Ok(node.wait_result())));
                }
                // self-edge: adds nothing reachable to the wait-for graph,
                // so `would_deadlock` never sees it
                Some(_) => self
                    .waiters
                    .entry(aid.clone())
                    .or_default()
                    .push(Waiter { caller: aid, done }),
            }
        }
    }
}

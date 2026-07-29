//! app/runtime-side commands: registration, id allocation, UI-event
//! forwarding, liveness pings, runtime attach/down

use anyhow::Result;
use tokio::sync::mpsc::error::TrySendError;

use super::free_variant;
use crate::agent::AgentId;
use crate::agent::handle::AgentEvent;
use crate::agent::handle::ExternalEvent;
use crate::agent::router::AgentRouter;
use crate::agent::router::RuntimeHandle;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::NodeStatus;
use crate::agent::router::graph::StatusPing;

impl AgentRouter {
    /// primary registration (`new_tab`/duplicate): root = own id
    pub fn handle_register_root(
        &mut self,
        aid: AgentId,
    ) -> Result<()> {
        anyhow::ensure!(!self.graph.contains_key(&aid), "agent {aid} already exists");
        self.all_ids.insert(aid.clone());
        let node = AgentNode::new(aid.clone(), None, NodeStatus::Spawning);
        let record = node.record(false);
        self.graph.insert(aid.clone(), node);
        drop(self.project.store().save_graph(&aid, &record));
        Ok(())
    }

    pub fn allocate(&mut self) -> AgentId {
        let aid = free_variant(&AgentId::generate_base(), &self.all_ids);
        self.all_ids.insert(aid.clone());
        aid
    }

    pub fn handle_forward(
        &mut self,
        aid: AgentId,
        event: ExternalEvent,
    ) -> Result<()> {
        let Some(node) = self.graph.get_mut(&aid) else {
            anyhow::bail!("agent {aid} is unreachable");
        };
        if node.status == NodeStatus::Dead {
            anyhow::bail!("agent {aid} is dead");
        }
        let Some(tx) = node.user_tx.clone() else {
            anyhow::bail!("agent {aid} runtime is not attached");
        };
        match tx.try_send(AgentEvent::External(event)) {
            Ok(()) => {
                node.delivered += 1;
                if node.status == NodeStatus::Idle {
                    node.status = NodeStatus::Running;
                }
                Ok(())
            }
            Err(TrySendError::Full(_)) => anyhow::bail!("agent {aid} mailbox is full"),
            Err(TrySendError::Closed(_)) => {
                self.fail_runtime(&aid, "agent runtime mailbox closed".into(), true);
                anyhow::bail!("agent {aid} is unreachable")
            }
        }
    }

    pub fn handle_status(
        &mut self,
        aid: AgentId,
        ping: StatusPing,
    ) {
        let Some(node) = self.graph.get_mut(&aid) else {
            return;
        };
        // A terminal node never accepts a late in-flight ping.
        if node.status == NodeStatus::Dead {
            return;
        }
        node.processed = ping.processed;
        if ping.outcome.output.is_some() {
            node.outcome.output = ping.outcome.output;
        }
        node.outcome.error = ping.outcome.error;
        // An idle ping older than a committed delivery is still effectively
        // running. The post-delivery ping is the one that may settle the node.
        node.status = if ping.status == NodeStatus::Idle && node.processed < node.delivered {
            NodeStatus::Running
        } else {
            ping.status
        };
        if node.status == NodeStatus::Idle {
            let result = node.wait_result();
            self.fire_waiters(&aid, Ok(result));
        }
    }

    pub fn handle_attach_runtime(
        &mut self,
        aid: AgentId,
        runtime: RuntimeHandle,
    ) -> Result<()> {
        let node = self
            .graph
            .get_mut(&aid)
            .ok_or_else(|| anyhow::anyhow!("agent {aid} is unreachable"))?;
        anyhow::ensure!(node.status != NodeStatus::Dead, "agent {aid} is dead");
        anyhow::ensure!(
            node.abort.is_none(),
            "agent {aid} runtime is already attached"
        );
        node.mailbox = Some(runtime.tx);
        node.user_tx = Some(runtime.user_tx);
        node.abort = Some(runtime.abort);
        Ok(())
    }

    pub fn handle_runtime_down(
        &mut self,
        aid: &AgentId,
        error: String,
    ) {
        self.fail_runtime(aid, error, false);
    }
}

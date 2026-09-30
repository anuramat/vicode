//! lifecycle: registration, id allocation, UI-event forwarding, status
//! reports, and the one way to start an agent — `launch` — whose runtime
//! runs supervised until its terminal `runtime_down`

use std::future::Future;
use std::panic::AssertUnwindSafe;

use anyhow::Result;
use futures::FutureExt;
use futures::future::AbortHandle;
use futures::future::AbortRegistration;
use futures::future::Abortable;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;

use super::free_variant;
use crate::agent::Agent;
use crate::agent::AgentId;
use crate::agent::event::AgentEvent;
use crate::agent::event::UserCommand;
use crate::agent::router::Router;
use crate::agent::router::RouterState;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::NodeStatus;
use crate::agent::router::graph::StatusReport;

impl Router {
    /// primary registration (`new_tab`/duplicate): root = own id
    pub fn register_root(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        let s = &mut *self.lock();
        anyhow::ensure!(!s.graph.contains_key(aid), "agent {aid} already exists");
        s.all_ids.insert(aid.clone());
        let node = AgentNode::new(aid.clone(), None, NodeStatus::Spawning);
        drop(s.project.store().save_graph(aid, &node.record(false)));
        s.graph.insert(aid.clone(), node);
        Ok(())
    }

    pub fn allocate_agent_id(&self) -> AgentId {
        self.lock().allocate()
    }

    pub fn forward(
        &self,
        aid: &AgentId,
        event: UserCommand,
    ) -> Result<()> {
        let s = &mut *self.lock();
        let Some(node) = s.graph.get_mut(aid) else {
            anyhow::bail!("agent {aid} is unreachable");
        };
        if node.status == NodeStatus::Dead {
            anyhow::bail!("agent {aid} is dead");
        }
        let Some(tx) = node.user_tx.clone() else {
            anyhow::bail!("agent {aid} runtime is not attached");
        };
        match tx.try_send(AgentEvent::User(event)) {
            Ok(()) => {
                node.delivered += 1;
                if node.status == NodeStatus::Idle {
                    node.status = NodeStatus::Running;
                }
                Ok(())
            }
            Err(TrySendError::Full(_)) => anyhow::bail!("agent {aid} mailbox is full"),
            Err(TrySendError::Closed(_)) => {
                s.fail_runtime(aid, "agent runtime mailbox closed".into(), true);
                anyhow::bail!("agent {aid} is unreachable")
            }
        }
    }

    pub fn status(
        &self,
        aid: &AgentId,
        report: StatusReport,
    ) {
        let s = &mut *self.lock();
        let Some(node) = s.graph.get_mut(aid) else {
            return;
        };
        // A terminal node never accepts a late in-flight report.
        if node.status == NodeStatus::Dead {
            return;
        }
        node.processed = report.processed;
        if report.outcome.output.is_some() {
            node.outcome.output = report.outcome.output;
        }
        node.outcome.error = report.outcome.error;
        // An idle report older than a committed delivery is still effectively
        // running. The post-delivery report is the one that may settle the node.
        node.status = if report.status == NodeStatus::Idle && node.processed < node.delivered {
            NodeStatus::Running
        } else {
            report.status
        };
        if node.status == NodeStatus::Idle {
            let result = node.wait_result();
            s.fire_waiters(aid, Ok(result));
        }
    }

    /// attach every agent to its registered node, then start them all —
    /// so none can reach a sibling that isn't attached yet
    pub fn launch(
        &self,
        agents: impl IntoIterator<Item = Agent>,
    ) -> Result<()> {
        let mut started = Vec::new();
        {
            let s = &mut *self.lock();
            for agent in agents {
                let (abort, registration) = AbortHandle::new_pair();
                s.attach(&agent.id, agent.tx.clone(), agent.user_tx.clone(), abort)?;
                started.push((agent, registration));
            }
        }
        for (agent, registration) in started {
            let aid = agent.id.clone();
            tokio::spawn(supervise(aid, self.clone(), agent.run(), registration));
        }
        Ok(())
    }

    pub fn runtime_down(
        &self,
        aid: &AgentId,
        error: String,
    ) {
        self.lock().fail_runtime(aid, error, false);
    }
}

impl RouterState {
    pub fn allocate(&mut self) -> AgentId {
        let aid = free_variant(&AgentId::generate_base(), &self.all_ids);
        self.all_ids.insert(aid.clone());
        aid
    }

    /// one-shot: a node's runtime is attached at most once per process
    pub fn attach(
        &mut self,
        aid: &AgentId,
        mailbox: Sender<AgentEvent>,
        user_tx: Sender<AgentEvent>,
        abort: AbortHandle,
    ) -> Result<()> {
        let node = self
            .graph
            .get_mut(aid)
            .ok_or_else(|| anyhow::anyhow!("agent {aid} is unreachable"))?;
        anyhow::ensure!(node.status != NodeStatus::Dead, "agent {aid} is dead");
        anyhow::ensure!(
            node.abort.is_none(),
            "agent {aid} runtime is already attached"
        );
        node.mailbox = Some(mailbox);
        node.user_tx = Some(user_tx);
        node.abort = Some(abort);
        Ok(())
    }
}

/// run an agent's loop to its terminal — return, error, panic or
/// cancellation — and report it as the node's death
pub async fn supervise(
    aid: AgentId,
    router: Router,
    future: impl Future<Output = Result<()>>,
    registration: AbortRegistration,
) {
    let outcome = AssertUnwindSafe(Abortable::new(future, registration))
        .catch_unwind()
        .await;
    let error = match outcome {
        Ok(Ok(Ok(()))) => "agent runtime exited unexpectedly".into(),
        Ok(Ok(Err(error))) => format!("agent runtime failed: {error:#}"),
        Ok(Err(_)) => "agent runtime cancelled unexpectedly".into(),
        Err(payload) => format!(
            "agent runtime panicked: {}",
            crate::agent::task::executor::panic_message(&*payload)
        ),
    };
    router.runtime_down(&aid, error);
}

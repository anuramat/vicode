//! lifecycle: id allocation, status reports, and the one way to start an
//! agent — `start` — whose runtime runs supervised until its terminal
//! `runtime_down`

use std::future::Future;
use std::panic::AssertUnwindSafe;

use anyhow::Result;
use futures::FutureExt;
use futures::future::AbortHandle;
use futures::future::AbortRegistration;
use futures::future::Abortable;

use super::free_variant;
use crate::agent::Agent;
use crate::agent::AgentId;
use crate::agent::router::Router;
use crate::agent::router::RouterState;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::NodeStatus;
use crate::agent::router::graph::Runtime;

impl Router {
    pub fn allocate_agent_id(&self) -> AgentId {
        self.lock().allocate()
    }

    /// informational: what `list` shows
    pub fn report_status(
        &self,
        aid: &AgentId,
        status: NodeStatus,
    ) {
        let s = &mut *self.lock();
        // a terminal node never accepts a late in-flight report
        if let Some(node) = s.graph.get_mut(aid)
            && let Runtime::Live {
                status: current, ..
            } = &mut node.runtime
        {
            *current = status;
        }
    }

    /// the one way to start a set-up agent: its node goes live under
    /// `parent` (or as a root), and its runtime starts once the graph record
    /// is durable — mail sent meanwhile queues in the agent's channel
    pub async fn start(
        &self,
        agent: Agent,
        parent: Option<&AgentId>,
    ) -> Result<()> {
        let aid = agent.id.clone();
        let (abort, registration) = AbortHandle::new_pair();
        let write = {
            let s = &mut *self.lock();
            let root = match parent {
                Some(parent) => s.child_root(parent)?,
                None => aid.clone(),
            };
            let node = AgentNode::live(root, parent.cloned(), agent.task_tx.clone(), abort);
            let write = s.project.store().save_graph(&aid, &node.record(false));
            s.graph.insert(aid.clone(), node);
            write
        };
        if let Err(error) = write.await {
            self.lock().drop_node(&aid);
            return Err(error);
        }
        Launch {
            agent,
            registration,
        }
        .go();
        Ok(())
    }

    /// A runtime failure is terminal for this process. The durable live
    /// graph record remains, so a full application restart retries it.
    pub fn runtime_down(
        &self,
        aid: &AgentId,
        error: String,
    ) {
        if let Some(node) = self.lock().graph.get_mut(aid)
            && matches!(node.runtime, Runtime::Live { .. })
        {
            node.runtime = Runtime::Dead(error);
        }
    }
}

impl RouterState {
    pub fn allocate(&mut self) -> AgentId {
        let aid = free_variant(&AgentId::generate_base(), &self.all_ids);
        self.all_ids.insert(aid.clone());
        aid
    }
}

/// an agent whose node is already live in the graph, its runtime not yet
/// started
pub struct Launch {
    pub agent: Agent,
    pub registration: AbortRegistration,
}

impl Launch {
    /// run the agent's loop supervised
    pub fn go(self) {
        let Self {
            agent,
            registration,
        } = self;
        let aid = agent.id.clone();
        let router = agent.router.clone();
        tokio::spawn(supervise(aid, router, agent.run(), registration));
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

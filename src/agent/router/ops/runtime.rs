//! lifecycle: id allocation, status reports, and the one way to create an
//! agent -- `create` -- whose runtime runs supervised until its terminal
//! `runtime_down`

use std::future::Future;
use std::panic::AssertUnwindSafe;

use anyhow::Result;
use anyhow::anyhow;
use futures::FutureExt;
use futures::future::AbortHandle;
use futures::future::AbortRegistration;
use futures::future::Abortable;

use super::free_variant;
use crate::agent::Agent;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::router::Router;
use crate::agent::router::RouterState;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::Runtime;
use crate::agent::task::panic_message;

impl Router {
    pub fn allocate_agent_id(&self) -> AgentId {
        self.lock().allocate()
    }

    /// informational: what `list` shows
    pub fn report_busy(
        &self,
        aid: &AgentId,
        busy: bool,
    ) {
        let s = &mut *self.lock();
        // a terminal node never accepts a late in-flight report
        if let Some(node) = s.graph.get_mut(aid)
            && let Runtime::Live { busy: current, .. } = &mut node.runtime
        {
            *current = busy;
        }
    }

    /// the one way to create an agent: `setup` prepares its workdir and
    /// returns its state, which is saved before the agent `start`s; any
    /// failure, panics included, rolls all of it back. Detached: the setup
    /// completes (or rolls back) even if the caller is cancelled
    pub async fn create(
        &self,
        aid: AgentId,
        parent: Option<AgentId>,
        setup: impl Future<Output = Result<AgentState>> + Send + 'static,
    ) -> Result<AgentState> {
        let router = self.clone();
        tokio::spawn(async move {
            let result = AssertUnwindSafe(async {
                let state = setup.await?;
                // durable graph record ⇒ durable state; a crash before the
                // record leaves residue for cleanup
                router.project.store().save_state(&aid, &state).await?;
                let agent = Agent::new(router.clone(), aid.clone(), state.clone());
                router.start(agent, parent.as_ref()).await?;
                Ok(state)
            })
            .catch_unwind()
            .await
            .unwrap_or_else(|panic| {
                Err(anyhow!("agent setup panicked: {}", panic_message(&*panic)))
            });
            if let Err(error) = &result
                && let Err(rollback) = router.rollback(&aid).await
            {
                return Err(anyhow!(
                    "{error:#}; failed to roll back {aid}: {rollback:#}"
                ));
            }
            result
        })
        .await?
    }

    /// start a set-up agent: its node goes live under `parent` (or as a
    /// root), and its runtime starts once the graph record is durable -- mail
    /// sent meanwhile queues in the agent's channel
    async fn start(
        &self,
        agent: Agent,
        parent: Option<&AgentId>,
    ) -> Result<()> {
        let aid = agent.id.clone();
        let (launch, write) = {
            let s = &mut *self.lock();
            let root = match parent {
                Some(parent) => s.child_root(parent)?,
                None => aid.clone(),
            };
            let (node, launch) = Launch::new(agent, root, parent.cloned());
            let write = self.project.store().save_graph(&aid, &node.record(false));
            s.graph.insert(aid.clone(), node);
            (launch, write)
        };
        if let Err(error) = write.await {
            self.lock().drop_node(&aid);
            return Err(error);
        }
        launch.go();
        Ok(())
    }

    /// undo a failed create: its node (if any), graph record, state and
    /// workdir
    async fn rollback(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        let write = {
            let s = &mut *self.lock();
            s.drop_node(aid);
            self.project.store().delete_agent(aid)
        };
        write.await?;
        self.project.delete_agent_workdir(aid).await
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
    /// a runtime for `agent`, not yet started, and the live node it
    /// reports to
    pub fn new(
        agent: Agent,
        root: AgentId,
        parent: Option<AgentId>,
    ) -> (AgentNode, Self) {
        let (abort, registration) = AbortHandle::new_pair();
        let node = AgentNode::live(root, parent, agent.task_tx.clone(), abort);
        (
            node,
            Self {
                agent,
                registration,
            },
        )
    }

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
        Err(payload) => format!("agent runtime panicked: {}", panic_message(&*payload)),
    };
    router.runtime_down(&aid, error);
}

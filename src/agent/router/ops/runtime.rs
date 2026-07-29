//! lifecycle: registration, id allocation, status reports, and the one way
//! to start an agent — `launch` — whose runtime runs supervised until its
//! terminal `runtime_down`

use std::future::Future;
use std::panic::AssertUnwindSafe;

use anyhow::Result;
use futures::FutureExt;
use futures::future::AbortHandle;
use futures::future::AbortRegistration;
use futures::future::Abortable;
use tokio::sync::mpsc::UnboundedReceiver;

use super::free_variant;
use crate::agent::Agent;
use crate::agent::AgentId;
use crate::agent::router::Router;
use crate::agent::router::RouterState;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::NodeStatus;
use crate::agent::router::graph::Runtime;
use crate::llm::history::message::UserMessage;

impl Router {
    /// primary registration (`new_tab`/duplicate): root = own id
    pub fn register_root(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        let s = &mut *self.lock();
        anyhow::ensure!(!s.graph.contains_key(aid), "agent {aid} already exists");
        s.all_ids.insert(aid.clone());
        let node = AgentNode::new(aid.clone(), None);
        drop(s.project.store().save_graph(aid, &node.record(false)));
        s.graph.insert(aid.clone(), node);
        Ok(())
    }

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

    /// the one way to start an agent: hand its registered node's mailbox to
    /// the runtime and run it supervised
    pub fn launch(
        &self,
        agent: Agent,
    ) -> Result<()> {
        let (abort, registration) = AbortHandle::new_pair();
        let mail = self.lock().go_live(&agent.id, abort)?;
        let aid = agent.id.clone();
        tokio::spawn(supervise(aid, self.clone(), agent.run(mail), registration));
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

    /// one-shot: take the pending mailbox, the node is live from now on
    pub fn go_live(
        &mut self,
        aid: &AgentId,
        abort: AbortHandle,
    ) -> Result<UnboundedReceiver<UserMessage>> {
        let node = self
            .graph
            .get_mut(aid)
            .ok_or_else(|| anyhow::anyhow!("agent {aid} is unreachable"))?;
        match node.runtime {
            Runtime::Pending(_) => {}
            Runtime::Live { .. } => anyhow::bail!("agent {aid} runtime is already attached"),
            Runtime::Dead(_) => anyhow::bail!("agent {aid} is dead"),
        }
        let live = Runtime::Live {
            abort,
            status: NodeStatus::Spawning,
        };
        let Runtime::Pending(mail) = std::mem::replace(&mut node.runtime, live) else {
            unreachable!("checked above");
        };
        Ok(mail)
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

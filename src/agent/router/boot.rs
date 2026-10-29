//! boot: rebuild the live graph from the store, and load every agent to
//! restore

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc::UnboundedSender;

use crate::agent::Agent;
use crate::agent::AgentId;
use crate::agent::router::Launch;
use crate::agent::router::Router;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::GraphRecord;
use crate::agent::router::graph::Runtime;
use crate::project::Project;
use crate::tui::app::AppEvent;

/// what [`Router::boot`] restored
pub struct Boot {
    pub router: Arc<Router>,
    /// the live roots, in id order; each one's state is in `agents`
    pub tabs: Vec<AgentId>,
    /// every loadable live agent, roots included: its node is live, its
    /// runtime is started by `go`
    pub agents: Vec<Launch>,
    /// the agents whose state failed to load
    pub failures: Vec<(AgentId, String)>,
}

impl Router {
    /// The graph table is authoritative: archived records stay out. Roots
    /// load first, then the children under valid roots -- state under an
    /// invalid root is never read (the root's failure covers it). An invalid
    /// root omits its whole tab; an invalid child stays reachable as a
    /// terminal `Dead` node, without preventing valid descendants from
    /// starting.
    pub async fn boot(
        app_tx: UnboundedSender<AppEvent>,
        project: Project,
    ) -> Result<Boot> {
        let store = project.store();
        let records = store.load_graph().await?;
        let mut all_ids = store.state_ids().await?;
        all_ids.extend(records.keys().cloned());
        let router = Self::new(app_tx, project.clone());
        let mut boot = Boot {
            router: router.clone(),
            tabs: Vec::new(),
            agents: Vec::new(),
            failures: Vec::new(),
        };
        let mut graph = HashMap::new();
        let (roots, children): (Vec<_>, Vec<_>) = records
            .into_iter()
            .filter(|(_, r)| !r.archived)
            .partition(|(_, r)| r.parent.is_none());
        // an invalid root omits its whole tab
        for (aid, record) in roots {
            if let Ok(node) = boot.restore(&aid, &record).await {
                boot.tabs.push(aid.clone());
                graph.insert(aid, node);
            }
        }
        // children under an invalid root are never read; an invalid child
        // stays as a terminal `Dead` node
        for (aid, record) in children {
            if !graph.contains_key(&record.root) {
                continue;
            }
            let node = boot
                .restore(&aid, &record)
                .await
                .unwrap_or_else(|error| AgentNode {
                    root: record.root,
                    parent: record.parent,
                    runtime: Runtime::Dead(error),
                });
            graph.insert(aid, node);
        }
        let s = &mut *router.lock();
        s.graph = graph;
        s.all_ids = all_ids;
        Ok(boot)
    }
}

impl Boot {
    /// load one agent's state; on success its launch is queued, on failure
    /// the error is recorded
    async fn restore(
        &mut self,
        aid: &AgentId,
        record: &GraphRecord,
    ) -> Result<AgentNode, String> {
        match self.router.project.store().load_state(aid).await {
            Ok(state) => {
                let agent = Agent::new(self.router.clone(), aid.clone(), state);
                let (node, launch) = Launch::new(agent, record.root.clone(), record.parent.clone());
                self.agents.push(launch);
                Ok(node)
            }
            Err(error) => {
                tracing::error!("failed to restore agent {aid}: {error:?}");
                let error = format!("{error:#}");
                self.failures.push((aid.clone(), error.clone()));
                Err(error)
            }
        }
    }
}

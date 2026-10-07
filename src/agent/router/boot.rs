//! boot: rebuild the live graph from the store, and load every agent to
//! restore

use std::collections::HashMap;

use anyhow::Result;
use futures::future::AbortHandle;
use tokio::sync::mpsc::UnboundedSender;

use crate::agent::Agent;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::router::Launch;
use crate::agent::router::Router;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::Runtime;
use crate::project::Project;
use crate::tui::app::AppEvent;

/// what [`Router::boot`] restored
pub struct Boot {
    pub router: Router,
    /// the live roots' states, in id order
    pub tabs: Vec<(AgentId, AgentState)>,
    /// every loadable live agent, roots included: its node is live, its
    /// runtime is started by `go`
    pub agents: Vec<Launch>,
    /// the agents whose state failed to load
    pub failures: Vec<(AgentId, String)>,
}

impl Router {
    /// The graph table is authoritative: archived records stay out. Roots
    /// load first, then the children under valid roots — state under an
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
        let router = Self::new(app_tx.clone(), project.clone());
        let mut boot = Boot {
            router: router.clone(),
            tabs: Vec::new(),
            agents: Vec::new(),
            failures: Vec::new(),
        };
        let mut graph = HashMap::new();
        // roots sort first (stably, so in id order): every child's root is
        // settled before the child
        let mut live: Vec<_> = records.into_iter().filter(|(_, r)| !r.archived).collect();
        live.sort_by_key(|(_, r)| r.parent.is_some());
        for (aid, record) in live {
            let is_root = record.parent.is_none();
            if !is_root && !graph.contains_key(&record.root) {
                continue;
            }
            let node = match store.load_state(&aid).await {
                Ok(state) => {
                    if is_root {
                        boot.tabs.push((aid.clone(), state.clone()));
                    }
                    let agent = Agent::new(
                        project.clone(),
                        router.clone(),
                        app_tx.clone(),
                        aid.clone(),
                        state,
                    );
                    let (abort, registration) = AbortHandle::new_pair();
                    let node =
                        AgentNode::live(record.root, record.parent, agent.task_tx.clone(), abort);
                    boot.agents.push(Launch {
                        agent,
                        registration,
                    });
                    node
                }
                Err(error) => {
                    tracing::error!("failed to restore agent {aid}: {error:?}");
                    let error = format!("{error:#}");
                    boot.failures.push((aid.clone(), error.clone()));
                    if is_root {
                        continue;
                    }
                    AgentNode {
                        root: record.root,
                        parent: record.parent,
                        runtime: Runtime::Dead(error),
                    }
                }
            };
            graph.insert(aid, node);
        }
        let s = &mut *router.lock();
        s.graph = graph;
        s.all_ids = all_ids;
        Ok(boot)
    }
}

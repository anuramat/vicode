use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;

use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;

use crate::agent::AgentId;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::WaitId;
use crate::agent::router::api::WaitResult;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::GraphRecord;
use crate::agent::router::graph::Runtime;
use crate::project::Project;
use crate::tui::app::AppEvent;

pub mod api;
pub mod graph;
mod ops;

// TODO move to config
/// per-tab limit on live agents
pub const TAB_AGENT_CAP: usize = 32;

/// live `wait` requests
#[derive(Debug)]
pub struct Waiter {
    pub id: WaitId,
    pub caller: AgentId,
    pub done: oneshot::Sender<Result<WaitResult, RouterError>>,
}

/// the agent graph; every operation is a plain method on [`Router`] that
/// runs under the lock — nothing awaits while holding it, async work (store
/// commits, workdir setup) runs in detached tails that lock again to finish
#[derive(Debug)]
pub struct RouterState {
    pub project: Project,
    /// handed to every agent the router spawns
    pub app_tx: Sender<AppEvent>,

    /// only live agents
    pub graph: HashMap<AgentId, AgentNode>,
    /// every allocated id including archived; stored so we can avoid collisions
    pub all_ids: BTreeSet<AgentId>,
    /// keyed by target
    pub waiters: HashMap<AgentId, Vec<Waiter>>,
    pub next_wait: u64,
}

#[derive(Clone, Debug)]
pub struct Router(Arc<Mutex<RouterState>>);

impl RouterState {
    /// `restored`: the boot-loaded agents to put in the graph — `None` to
    /// be launched, `Some(error)` if unloadable (a terminal `Dead` node)
    pub fn start(
        app_tx: Sender<AppEvent>,
        project: Project,
        records: BTreeMap<AgentId, GraphRecord>,
        state_ids: BTreeSet<AgentId>,
        mut restored: HashMap<AgentId, Option<String>>,
    ) -> Router {
        let mut all_ids = state_ids;
        all_ids.extend(records.keys().cloned());
        let graph = records
            .into_iter()
            .filter_map(|(aid, record)| {
                if record.archived {
                    return None;
                }
                let error = restored.remove(&aid)?;
                let mut node = AgentNode::new(record.root, record.parent);
                if let Some(error) = error {
                    node.runtime = Runtime::Dead(error);
                }
                Some((aid, node))
            })
            .collect();
        Router(Arc::new(Mutex::new(Self {
            project,
            app_tx,
            graph,
            all_ids,
            waiters: HashMap::new(),
            next_wait: 0,
        })))
    }
}

impl Router {
    fn lock(&self) -> MutexGuard<'_, RouterState> {
        self.0.lock().expect("router lock poisoned")
    }
}

mod tests;

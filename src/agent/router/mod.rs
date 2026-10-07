use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;

use tokio::sync::mpsc::UnboundedSender;

use crate::agent::AgentId;
use crate::agent::router::graph::AgentNode;
use crate::project::Project;
use crate::tui::app::AppEvent;

pub mod api;
pub mod boot;
pub mod graph;
mod ops;

pub use ops::runtime::Launch;

// TODO move to config
/// per-tab limit on live agents
pub const TAB_AGENT_CAP: usize = 32;

/// the agent graph; every operation is a plain method on [`Router`] that
/// runs under the lock — nothing awaits while holding it, async work (store
/// commits, workdir setup) runs in detached tails that lock again to finish
#[derive(Debug)]
pub struct RouterState {
    pub project: Project,
    /// handed to every agent the router spawns
    pub app_tx: UnboundedSender<AppEvent>,

    /// only live agents
    pub graph: HashMap<AgentId, AgentNode>,
    /// every allocated id including archived; stored so we can avoid collisions
    pub all_ids: BTreeSet<AgentId>,
}

#[derive(Clone, Debug)]
pub struct Router(Arc<Mutex<RouterState>>);

impl Router {
    /// an empty graph; [`Router::boot`] restores the saved one
    pub fn new(
        app_tx: UnboundedSender<AppEvent>,
        project: Project,
    ) -> Self {
        Self(Arc::new(Mutex::new(RouterState {
            project,
            app_tx,
            graph: HashMap::new(),
            all_ids: BTreeSet::new(),
        })))
    }

    fn lock(&self) -> MutexGuard<'_, RouterState> {
        self.0.lock().expect("router lock poisoned")
    }
}

mod tests;

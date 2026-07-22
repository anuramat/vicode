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

#[derive(Debug)]
pub struct Router {
    pub project: Project,
    pub app_tx: UnboundedSender<AppEvent>,
    state: Mutex<RouterState>,
}

#[derive(Debug, Default)]
pub struct RouterState {
    /// only live agents
    pub graph: HashMap<AgentId, AgentNode>,
    /// every allocated id including archived; stored so we can avoid collisions
    pub all_ids: BTreeSet<AgentId>,
}

impl Router {
    /// an empty graph; [`Router::boot`] restores the saved one
    pub fn new(
        app_tx: UnboundedSender<AppEvent>,
        project: Project,
    ) -> Arc<Self> {
        Arc::new(Self {
            project,
            app_tx,
            state: Mutex::default(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, RouterState> {
        self.state.lock().expect("router lock poisoned")
    }
}

mod tests;

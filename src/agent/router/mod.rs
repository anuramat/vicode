use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use anyhow::Result;
use futures::future::AbortHandle;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio::sync::oneshot;

use crate::agent::AgentId;
use crate::agent::handle::AgentEvent;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::TurnOutcome;
use crate::agent::router::api::WaitResult;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::GraphRecord;
use crate::agent::router::graph::NodeStatus;
use crate::project::Project;
use crate::tui::app::AppEvent;

pub mod api;
mod client;
mod command;
pub mod graph;
mod handle;

pub use command::RouterCommand;

// TODO add docstring
const CHANNEL_CAPACITY: usize = 100;

// TODO move to config
/// per-tab limit on live agents
pub const TAB_AGENT_CAP: usize = 32;

#[derive(Debug)]
pub struct RuntimeHandle {
    /// inter-agent + task mailbox
    tx: Sender<AgentEvent>,
    /// dedicated channel for UI events
    user_tx: Sender<AgentEvent>,
    abort: AbortHandle,
}

impl RuntimeHandle {
    pub fn new(
        tx: Sender<AgentEvent>,
        user_tx: Sender<AgentEvent>,
        abort: AbortHandle,
    ) -> Self {
        Self { tx, user_tx, abort }
    }
}

/// live `wait` requests
#[derive(Debug)]
pub struct Waiter {
    pub caller: AgentId,
    pub done: oneshot::Sender<Result<WaitResult, RouterError>>,
}

pub struct AgentRouter {
    pub project: Project,

    rx: Receiver<RouterCommand>,
    pub handle: AgentRouterHandle,

    /// only live agents
    pub graph: HashMap<AgentId, AgentNode>,
    /// every allocated id including archived; stored so we can avoid collisions
    pub all_ids: BTreeSet<AgentId>,
    /// keyed by target
    pub waiters: HashMap<AgentId, Vec<Waiter>>,
}

#[derive(Clone, Debug)]
pub struct AgentRouterHandle {
    tx: Sender<RouterCommand>,
    app_tx: Sender<AppEvent>,
}

impl AgentRouter {
    pub fn spawn(
        app_tx: Sender<AppEvent>,
        project: Project,
        records: BTreeMap<AgentId, GraphRecord>,
        state_ids: BTreeSet<AgentId>,
        mut outcomes: HashMap<AgentId, TurnOutcome>,
    ) -> AgentRouterHandle {
        let mut all_ids = state_ids;
        all_ids.extend(records.keys().cloned());
        let graph = records
            .into_iter()
            .filter_map(|(aid, record)| {
                if record.archived {
                    return None;
                }
                let outcome = outcomes.remove(&aid)?;
                let mut node = AgentNode::new(
                    record.root,
                    record.parent,
                    if outcome.error.is_some() {
                        NodeStatus::Dead
                    } else {
                        NodeStatus::Spawning
                    },
                );
                node.outcome = outcome;
                Some((aid, node))
            })
            .collect();
        Self::start(app_tx, project, graph, all_ids)
    }

    fn start(
        app_tx: Sender<AppEvent>,
        project: Project,
        graph: HashMap<AgentId, AgentNode>,
        all_ids: BTreeSet<AgentId>,
    ) -> AgentRouterHandle {
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        let handle = AgentRouterHandle { tx, app_tx };
        let router = Self {
            project,
            graph,
            all_ids,
            waiters: HashMap::new(),
            rx,
            handle: handle.clone(),
        };
        tokio::spawn(router.run());
        handle
    }

    async fn run(mut self) {
        while let Some(cmd) = self.rx.recv().await {
            self.dispatch(cmd);
        }
    }
}

mod tests;

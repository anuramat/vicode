pub mod handle;
pub mod keys;
pub mod render;
pub mod run;
pub mod tabs;

use std::collections::HashMap;

use anyhow::Result;
use git2::Repository;
pub use handle::AppEvent;
use indexmap::IndexMap;
use ratatui::DefaultTerminal;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio::task::JoinSet;

use crate::agent::AgentEvent;
use crate::agent::AgentId;
use crate::agent::handle::ParentEvent;
use crate::project::PROJECT;
use crate::tui::tab::Tab;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::tablist::TabList;

pub struct App<'a> {
    pub should_exit: bool,

    pub rx: Receiver<AppEvent>,
    pub tx: Sender<AppEvent>,
    /// agents, event translation
    pub joinset: JoinSet<Result<()>>,

    /// hide tool calls, etc
    pub ctx: RenderContext,
    /// true if we received an event but didn't redraw yet
    pub dirty: bool,
    pub terminal: DefaultTerminal,

    /// channel for primary agents to report to
    pub parent_tx: Sender<ParentEvent>,
    /// primary agents
    pub agents: HashMap<AgentId, Sender<AgentEvent>>,
    /// UI for primary agents
    pub tabs: IndexMap<AgentId, Tab<'a>>,

    /// project name shown in status line
    pub project_name: String,
    pub tablist: TabList<'a>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    primary_agents: Vec<AgentId>,
}

const CHANNEL_CAPACITY: usize = 100;

impl<'a> App<'a> {
    async fn new() -> Result<Self> {
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        let (parent_tx, parent_rx) = channel(CHANNEL_CAPACITY);
        let mut joinset = JoinSet::new();
        joinset.spawn(crate::tui::app::tabs::translate_agent_events(
            tx.clone(),
            parent_rx,
        ));

        let project_name = crate::project::PROJECT
            .root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        Ok(Self {
            project_name,
            ctx: Default::default(),
            dirty: true,
            tx,
            rx,
            parent_tx,
            should_exit: false,
            tablist: TabList::default(),
            tabs: IndexMap::new(),
            agents: HashMap::new(),
            terminal: ratatui::init(),
            joinset,
        })
    }

    pub fn state(&self) -> AppState {
        AppState {
            primary_agents: self.tabs.keys().cloned().collect(),
        }
    }
}

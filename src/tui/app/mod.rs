pub mod handle;
pub mod keys;
pub mod render;
pub mod run;
pub mod tabs;

use std::collections::HashMap;

use anyhow::Result;
pub use handle::AppEvent;
use indexmap::IndexMap;
use ratatui::DefaultTerminal;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio::task::JoinSet;
use tokio::time::Duration;
use tokio::time::Instant;

use crate::agent::AgentEvent;
use crate::agent::id::AgentId;
use crate::agent::handle::ParentEvent;
use crate::tui::tab::Tab;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::tablist::TabList;

#[derive(Clone, Copy)]
pub enum NotificationKind {
    Info,
    Error,
}

pub struct Notification {
    pub kind: NotificationKind,
    pub msg: String,
    pub expires_at: Instant,
}

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

    /// channel for primary agents to report to
    pub parent_tx: Sender<ParentEvent>,
    /// primary agents
    pub agents: HashMap<AgentId, Sender<AgentEvent>>,
    /// UI for primary agents
    pub tabs: IndexMap<AgentId, Tab<'a>>,

    /// project name shown in status line
    pub project_name: String,
    pub notification: Option<Notification>,
    pub tablist: TabList<'a>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    primary_agents: Vec<AgentId>,
}

const CHANNEL_CAPACITY: usize = 100;
const NOTIFICATION_DURATION: Duration = Duration::from_secs(1);

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
            notification: None,
            joinset,
        })
    }

    pub fn state(&self) -> AppState {
        AppState {
            primary_agents: self.tabs.keys().cloned().collect(),
        }
    }

    pub fn selected_aid(&self) -> Option<AgentId> {
        self.selected_tab()
            .and_then(|idx| self.tabs.get_index(idx))
            .map(|(aid, _)| aid.clone())
    }

    pub fn notify(
        &mut self,
        kind: NotificationKind,
        msg: String,
    ) {
        self.notification = Some(Notification {
            kind,
            msg,
            expires_at: Instant::now() + NOTIFICATION_DURATION,
        });
    }
}

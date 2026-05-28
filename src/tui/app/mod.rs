pub mod command;
pub mod handle;
pub mod key;
pub mod run;
pub mod tabs;

use std::collections::HashSet;

use anyhow::Result;
use crossterm::event::KeyEvent;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio::time::Duration;
use tokio::time::Instant;

use crate::agent::handle::ParentEvent;
use crate::agent::id::AgentId;
use crate::agent::router::AgentRouter;
use crate::agent::router::AgentRouterHandle;
use crate::project::Project;
use crate::tui::tab::TabEntry;
use crate::tui::widgets::cmdline::Cmdline;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::tablist::TabList;

#[derive(Clone, Copy)]
pub enum NotificationKind {
    Info,
    Error,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AppFocus {
    Body,
    Info,
    #[default]
    Tabs,
}

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Paste(String),

    NewAgent(AgentId),
    ParentEvent(AgentId, ParentEvent),
    TabStatusChanged(AgentId),

    Redraw,
}

pub struct Notification {
    pub kind: NotificationKind,
    pub msg: String,
    pub expires_at: Instant,
}

pub struct App<'a> {
    pub project: Project,
    pub should_exit: bool,

    pub rx: Receiver<AppEvent>,
    pub tx: Sender<AppEvent>,
    pub router: AgentRouterHandle,

    /// hide tool calls, etc
    pub ctx: RenderContext,
    /// true if we received an event but didn't redraw yet
    pub dirty: bool,

    /// UI for primary agents
    pub tabs: IndexMap<AgentId, TabEntry<'a>>,

    /// project name shown in status line
    pub project_name: String,
    pub cmdline: Cmdline<'a>,
    pub notification: Option<Notification>,
    pub tablist: TabList<'a>,
    pub focus: AppFocus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    visible_order: Vec<AgentId>,
}

const CHANNEL_CAPACITY: usize = 100;
// TODO make configurable
const NOTIFICATION_DURATION: Duration = Duration::from_secs(1);

impl App<'_> {
    fn new(
        project: Project,
        agent_ids: HashSet<AgentId>,
    ) -> Self {
        // TODO figure out what should stay here, and what belongs to run()/launch()
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        let router = AgentRouter::spawn(tx.clone(), project.clone(), agent_ids);

        let project_name = project.name();
        let ctx = project.config().render;

        Self {
            project,
            focus: AppFocus::default(),
            project_name,
            cmdline: Cmdline::new(),
            ctx,
            dirty: true,
            tx,
            rx,
            router,
            should_exit: false,
            tablist: TabList::default(),
            tabs: IndexMap::new(),
            notification: None,
        }
    }

    pub fn state(&self) -> AppState {
        AppState {
            visible_order: self.tabs.keys().cloned().collect(),
        }
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

    pub async fn save_app_state(&self) -> Result<()> {
        self.project.store().save_app(&self.state()).await
    }
}

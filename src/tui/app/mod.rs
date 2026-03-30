pub mod handle;
pub mod keys;
pub mod render;
pub mod run;
pub mod tabs;

use anyhow::Result;
pub use handle::AppEvent;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio::time::Duration;
use tokio::time::Instant;

use crate::agent::id::AgentId;
use crate::config::CONFIG;
use crate::project::PROJECT;
use crate::project::layout::LayoutTrait;
use crate::tui::tab::TabEntry;
use crate::tui::widgets::cmdline::Cmdline;
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
    pub show_tabs: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    primary_agents: Vec<AgentId>,
}

const CHANNEL_CAPACITY: usize = 100;
// TODO make configurable
const NOTIFICATION_DURATION: Duration = Duration::from_secs(1);

impl<'a> App<'a> {
    async fn new() -> Result<Self> {
        // TODO figure out what should stay here, and what belongs to run()/launch()
        let (tx, rx) = channel(CHANNEL_CAPACITY);

        let project_name = PROJECT.name();

        Ok(Self {
            show_tabs: false,
            project_name,
            cmdline: Cmdline::default(),
            ctx: CONFIG.render,
            dirty: true,
            tx,
            rx,
            should_exit: false,
            tablist: TabList::default(),
            tabs: IndexMap::new(),
            notification: None,
        })
    }

    pub fn state(&self) -> AppState {
        AppState {
            primary_agents: self.tabs.keys().cloned().collect(),
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
        let data = self.state();
        let serialized = serde_json::to_string_pretty(&data)?;
        let path = PROJECT.app_state();
        tokio::fs::write(path, serialized).await?;
        Ok(())
    }

    pub async fn load_app_state() -> Result<AppState> {
        let path = PROJECT.app_state();
        if !path.exists() {
            return Ok(AppState::default());
        }
        let serialized = tokio::fs::read_to_string(path).await?;
        Ok(serde_json::from_str(&serialized)?)
    }
}

pub mod command;
pub mod handle;
pub mod key;
mod render_tests;
pub mod run;
pub mod tabs;

use anyhow::Result;
use crossterm::event::KeyEvent;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Duration;
use tokio::time::Instant;

use crate::agent::event::UiEvent;
use crate::agent::id::AgentId;
use crate::agent::router::Router;
use crate::project::Project;
use crate::tui::tab::Tab;
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

/// events on the app channel — external sources only (the crossterm
/// translator and agents); the app loop
/// itself never sends here, so it can never block on its own bounded
/// queue
#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Paste(String),

    Agent(AgentId, UiEvent),

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

    pub rx: UnboundedReceiver<AppEvent>,
    pub tx: UnboundedSender<AppEvent>,
    pub router: Router,

    /// hide tool calls, etc
    pub ctx: RenderContext,
    /// true if we received an event but didn't redraw yet
    pub dirty: bool,

    /// UI for primary agents
    pub tabs: IndexMap<AgentId, Tab<'a>>,

    /// project name shown in status line
    pub project_name: String,
    pub cmdline: Cmdline<'a>,
    pub notification: Option<Notification>,
    pub tablist: TabList<'a>,
    pub focus: AppFocus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    pub visible_order: Vec<AgentId>,
}

// TODO make configurable
const NOTIFICATION_DURATION: Duration = Duration::from_secs(1);

impl App<'_> {
    fn with_router(
        project: Project,
        tx: UnboundedSender<AppEvent>,
        rx: UnboundedReceiver<AppEvent>,
        router: Router,
    ) -> Self {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;

    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::agent::router::RouterState;
    use crate::agent::router::graph::GraphRecord;

    impl App<'_> {
        pub fn new(
            project: Project,
            state_ids: BTreeSet<AgentId>,
            records: BTreeMap<AgentId, GraphRecord>,
        ) -> Self {
            // TODO figure out what should stay here, and what belongs to run()/launch()
            let (tx, rx) = unbounded_channel();
            let restored = records.keys().map(|a| (a.clone(), None)).collect();
            let router =
                RouterState::start(tx.clone(), project.clone(), records, state_ids, restored);
            Self::with_router(project, tx, rx, router)
        }
    }
}

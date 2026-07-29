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
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::time::Duration;
use tokio::time::Instant;

use crate::agent::handle::ParentEvent;
use crate::agent::id::AgentId;
use crate::agent::router::AgentRouterHandle;
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
/// translator, agents, and detached watcher tasks); the app loop
/// itself never sends here, so it can never block on its own bounded
/// queue (H1)
#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Paste(String),

    ParentEvent(AgentId, ParentEvent),

    /// the duplicate watcher saw the ack channel close unresolved: roll back
    /// the copy's preview tab (M5)
    DuplicateFailed(AgentId),

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

const CHANNEL_CAPACITY: usize = 100;
// TODO make configurable
const NOTIFICATION_DURATION: Duration = Duration::from_secs(1);

impl App<'_> {
    fn with_router(
        project: Project,
        tx: Sender<AppEvent>,
        rx: Receiver<AppEvent>,
        router: AgentRouterHandle,
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

    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::agent::router::AgentRouter;
    use crate::agent::router::graph::GraphRecord;

    impl App<'_> {
        pub fn new(
            project: Project,
            state_ids: BTreeSet<AgentId>,
            records: BTreeMap<AgentId, GraphRecord>,
        ) -> Self {
            // TODO figure out what should stay here, and what belongs to run()/launch()
            let (tx, rx) = channel(CHANNEL_CAPACITY);
            let outcomes = records
                .keys()
                .map(|a| (a.clone(), Default::default()))
                .collect();
            let router =
                AgentRouter::spawn(tx.clone(), project.clone(), records, state_ids, outcomes);
            Self::with_router(project, tx, rx, router)
        }
    }
}

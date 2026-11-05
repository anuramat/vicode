//! the selected tab's workdir views (info pane, `@` completion), read off
//! the app loop: the agent may change its workdir at any time

use std::future::pending;

use anyhow::Result;
use tokio::task::JoinHandle;

use crate::agent::id::AgentId;
use crate::tui::app::App;
use crate::tui::tab::TabRuntime;
use crate::tui::tab::input::tracked_files;
use crate::tui::widgets::info::InfoWidget;
use crate::tui::widgets::info::read_info;

pub struct WorkdirViews {
    aid: AgentId,
    /// `info_cmd` stdout: the widget itself isn't `Send`
    info: Vec<u8>,
    files: Vec<String>,
}

pub type RefreshTask = JoinHandle<Result<WorkdirViews>>;

impl App<'_> {
    /// (re)start reading the selected tab's workdir; replaces the read in
    /// flight
    pub fn refresh(&mut self) {
        let Ok(tab) = self.selected_tab() else {
            return;
        };
        // not started (the workdir may not be mounted yet) or dead
        if !matches!(tab.runtime, TabRuntime::Live(_)) {
            return;
        }
        let (aid, project) = (tab.aid.clone(), self.project.clone());
        let task = tokio::spawn(async move {
            let workdir = project.agent_workdir(&aid);
            let files = tokio::task::spawn_blocking(move || tracked_files(&workdir)).await??;
            let info = read_info(&project, &aid).await?;
            Ok(WorkdirViews { aid, info, files })
        });
        if let Some(old) = self.refreshing.replace(task) {
            old.abort();
        }
    }

    pub fn apply_refresh(
        &mut self,
        views: WorkdirViews,
    ) -> Result<()> {
        // the tab may be closed by now
        let Some(tab) = self.tabs.get_mut(&views.aid) else {
            return Ok(());
        };
        tab.info = InfoWidget::new(&views.info)?;
        tab.set_file_completion(views.files)
    }
}

/// the read in flight, or never
pub async fn join_refresh(task: &mut Option<RefreshTask>) -> Result<WorkdirViews> {
    match task {
        Some(task) => task.await?,
        None => pending().await,
    }
}

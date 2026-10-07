//! the selected tab's workdir views (info pane, `@` completion), read off
//! the app loop: the agent may change its workdir at any time

use std::future::pending;

use anyhow::Result;
use tokio::task::JoinHandle;

use crate::agent::id::AgentId;
use crate::tui::app::App;
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
        // not started yet: the workdir may not be mounted
        if tab.control.is_none() {
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

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use git2::Repository;
    use similar_asserts::assert_eq;

    use super::*;
    use crate::agent::AgentState;
    use crate::project::Project;
    use crate::tui::tab::Tab;

    /// an app with one selected tab, on a workdir tracking `src.rs`
    fn app(started: bool) -> (App<'static>, AgentId) {
        let mut app = App::new(Project::new_test().unwrap().0);
        let aid = AgentId::from("tab-refresh".to_string());
        let workdir = app.project.agent_workdir(&aid);
        std::fs::create_dir_all(&workdir).unwrap();
        let repo = Repository::init(&workdir).unwrap();
        std::fs::write(workdir.join("src.rs"), "").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("src.rs")).unwrap();
        index.write().unwrap();
        let control = started.then(|| tokio::sync::mpsc::unbounded_channel().0);
        let project = app.project.clone();
        let tab = Tab::new(control, aid.clone(), AgentState::fake(), &project);
        app.tabs.insert(aid.clone(), tab);
        app.rebuild_tablist();
        app.select_tab(Some(0));
        (app, aid)
    }

    #[tokio::test]
    async fn refresh_reads_workdir_views() {
        let (mut app, aid) = app(true);

        app.refresh();
        let views = join_refresh(&mut app.refreshing).await.unwrap();
        assert!(String::from_utf8_lossy(&views.info).contains("src.rs"));
        app.apply_refresh(views).unwrap();

        let tab = app.tabs.get_mut(&aid).unwrap();
        tab.insert_mode(true);
        for ch in "@sr".chars() {
            tab.key_insert(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        tab.input.completion_next();
        assert_eq!(tab.input.textarea.lines(), ["@src.rs"]);
    }

    #[tokio::test]
    async fn refresh_skips_unstarted_tab() {
        let (mut app, _) = app(false);
        app.refresh();
        assert!(app.refreshing.is_none());
    }

    #[tokio::test]
    async fn refresh_for_closed_tab_is_dropped() {
        let (mut app, aid) = app(true);
        app.refresh();
        let views = join_refresh(&mut app.refreshing).await.unwrap();
        app.tabs.shift_remove(&aid);
        app.apply_refresh(views).unwrap();
    }
}

use anyhow::Result;
use git2::Repository;
use indexmap::IndexMap;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentState;
use crate::agent::handle::ExternalEvent;
use crate::agent::id::AgentId;
use crate::tui::app::App;
use crate::tui::app::AppEvent;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::Tab;

impl<'a> App<'a> {
    /// rebuild tablist widget
    pub fn rebuild_tablist(&mut self) {
        self.tablist.rebuild(&self.tabs);
        self.select_tab(self.selected_tab_idx());
    }

    pub async fn load_tabs(
        &mut self,
        tab_agents: Vec<(AgentId, AgentState)>,
        agents: Vec<(AgentId, AgentState)>,
    ) -> Result<()> {
        let mut tabs = IndexMap::new();
        for (aid, state) in &tab_agents {
            tabs.insert(
                aid.clone(),
                Tab::new(None, aid.clone(), state.clone(), &self.project),
            );
        }
        self.tabs = tabs;
        self.rebuild_tablist();

        let mut tasks = Vec::new();
        for (aid, state) in agents {
            let agent = Agent::new(
                self.project.clone(),
                self.router.clone(),
                aid.clone(),
                state,
            );
            let (runtime, task) = agent.prepare();
            self.router.attach_runtime(aid, runtime).await?;
            tasks.push(task);
        }
        for task in tasks {
            task.launch();
        }
        Ok(())
    }

    /// create a new primary agent, and a corresponding tab
    pub async fn new_tab(&mut self) -> Result<()> {
        let aid = self.router.allocate_agent_id().await?;
        let repo = Repository::discover(self.project.root())?;
        let commit = repo.head()?.peel_to_commit()?.id().to_string();
        let instructions = self.project.instructions(&aid).await?;
        let assistant = self.project.assistants().primary()?;
        let state = AgentState::new(assistant.id, commit, instructions);
        self.new_agent(aid.clone(), state.clone()).await?;
        self.insert_tab(aid, state);
        Ok(())
    }

    /// insert after the selected tab and select it
    fn insert_tab(
        &mut self,
        aid: AgentId,
        state: AgentState,
    ) {
        let idx = self.selected_tab_idx().map_or(self.tabs.len(), |x| x + 1);
        let tab = Tab::new(None, aid.clone(), state, &self.project);
        self.tabs.shift_insert(idx, aid, tab);
        self.select_tab(Some(idx));
        self.rebuild_tablist();
    }

    pub async fn new_agent(
        &self,
        aid: AgentId,
        state: AgentState,
    ) -> Result<()> {
        self.project
            .new_agent_workdir(&state.context.commit, &aid)
            .await?;
        let agent = Agent::new(self.project.clone(), self.router.clone(), aid, state);
        agent.launch_root().await
    }

    pub async fn handle_started(
        &mut self,
        aid: &AgentId,
        state: AgentState,
    ) -> Result<()> {
        let router = self.router.clone();
        let tab = self.tab_mut_by_aid(aid)?;
        tab.state = state;
        // a (re)start's fresh runtime has no in-flight calls; the prior
        // runtime's tee'd output is stale render state and the following
        // deduplicated StatusUpdate(Idle) would skip set_state's clear (L7)
        tab.live_output.clear();
        tab.refresh_assistant_config();
        tab.router = Some(router);
        tab.refresh_file_completion()?;
        tab.refresh_info().await?;
        self.rebuild_tablist();
        self.save_app_state().await?;
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn duplicate_tab(&mut self) -> Result<()> {
        let original = self.selected_tab()?;
        let Some(router) = original.router.clone() else {
            return Ok(());
        };
        let original_aid = original.aid.clone();
        let state = original.state.clone();

        let aid = self.router.allocate_agent_id().await?;
        self.insert_tab(aid.clone(), state);

        // the ack resolves iff the copy registered; every failure path just
        // drops the sender, so the watcher rolls the preview back (M5)
        let (ack, ack_rx) = tokio::sync::oneshot::channel();
        let (tx, copy) = (self.tx.clone(), aid.clone());
        tokio::spawn(async move {
            if ack_rx.await.is_err() {
                drop(tx.send(AppEvent::DuplicateFailed(copy)).await);
            }
        });
        router
            .forward(
                original_aid,
                ExternalEvent::DuplicateRequest { copy: aid, ack },
            )
            .await?;
        Ok(())
    }

    /// archive selected tab: every member goes `Unreachable`, graph records
    /// flip to `archived`, mounts release — state + workdirs retained (§2.5)
    pub async fn archive_tab(&mut self) -> Result<()> {
        let idx = self
            .selected_tab_idx()
            .ok_or_else(|| anyhow::anyhow!("no tab selected"))?;
        let (_, tab) = self
            .tabs
            .get_index(idx)
            .ok_or_else(|| anyhow::anyhow!("tab with idx {idx} not found"))?;
        tab.router()?;
        let (aid, _) = self
            .tabs
            .shift_remove_index(idx)
            .ok_or_else(|| anyhow::anyhow!("tab with idx {idx} not found"))?;
        self.router.archive_tab(aid).await?;
        self.rebuild_tablist();
        self.save_app_state().await?;
        Ok(())
    }

    pub fn selected_tab_idx(&self) -> Option<usize> {
        let n_tabs = self.tabs.len();
        if n_tabs == 0 {
            return None;
        }
        self.tablist.selected().map(|s| s.min(n_tabs - 1))
    }

    pub fn selected_tab(&self) -> Result<&Tab<'a>> {
        let Some(idx) = self.selected_tab_idx() else {
            anyhow::bail!("no tab selected");
        };
        let Some((_, tab)) = self.tabs.get_index(idx) else {
            anyhow::bail!("tab not found");
        };
        Ok(tab)
    }

    pub fn tab_mut_by_aid(
        &mut self,
        aid: &AgentId,
    ) -> Result<&mut Tab<'a>> {
        let Some(tab) = self.tabs.get_mut(aid) else {
            anyhow::bail!("tab not found");
        };
        Ok(tab)
    }

    pub fn selected_tab_mut(&mut self) -> Result<&mut Tab<'a>> {
        let Some(idx) = self.selected_tab_idx() else {
            anyhow::bail!("no tab selected");
        };
        let Some((_, tab)) = self.tabs.get_index_mut(idx) else {
            anyhow::bail!("tab not found");
        };
        Ok(tab)
    }

    pub fn next_tab(&mut self) {
        let Some(idx) = self.selected_tab_idx() else {
            self.select_tab(Some(0));
            return;
        };
        self.select_tab(Some(idx.checked_add(1).expect("tab index overflow")));
    }

    pub fn prev_tab(&mut self) {
        let Some(idx) = self.selected_tab_idx() else {
            self.last_tab();
            return;
        };
        self.select_tab(idx.checked_sub(1));
    }

    /// select a tab, checking the index
    pub fn select_tab(
        &mut self,
        mut idx: Option<usize>,
    ) -> Option<usize> {
        idx = idx.and_then(|i| {
            let n_tabs = self.tabs.len();
            if n_tabs == 0 || i >= n_tabs {
                set_osc7(self.project.root());
                None
            } else {
                if let Some((_, tab)) = self.tabs.get_index(i) {
                    tab.set_osc7();
                }
                Some(i)
            }
        });
        self.tablist.select(idx);
        idx
    }

    fn last_tab(&mut self) {
        let last = self.tabs.len().checked_sub(1);
        self.select_tab(last);
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;

    #[tokio::test]
    async fn new_tab_creates_agent_and_tab() {
        let mut app = App::new(
            crate::project::Project::new_test().unwrap().0,
            Default::default(),
            Default::default(),
        );

        app.new_tab().await.unwrap();

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.selected_tab_idx(), Some(0));
        let (tab_aid, tab) = app.tabs.get_index(0).unwrap();
        assert!(!tab.state.context.commit.is_empty());
        let instructions = app.project.instructions(tab_aid).await.unwrap();
        assert_eq!(tab.state.context.history.instructions(), instructions);
        // the agent is real: state saved, runtime registered with the router
        let saved = app.project.store().load_state(tab_aid).await.unwrap();
        assert_eq!(saved.context.commit, tab.state.context.commit);
        // a primary pins its own base too, so its inspect base does not
        // depend on `vc-<aid>` still pointing at the same commit
        let repo = git2::Repository::open(app.project.root()).unwrap();
        assert_eq!(
            repo.find_reference(&app.project.base_ref(tab_aid))
                .unwrap()
                .target()
                .unwrap()
                .to_string(),
            saved.context.base
        );
        app.router.shutdown(tab_aid.clone()).await.unwrap();
    }

    #[tokio::test]
    async fn archive_tab_stops_agent_but_keeps_workdir() {
        use futures::future::AbortHandle;
        use tokio::sync::mpsc::channel;

        use crate::agent::router::RuntimeHandle;

        let project = crate::project::Project::new_test().unwrap().0;
        let mut app = App::new(project.clone(), Default::default(), Default::default());
        let aid = AgentId::from(format!("archive-me-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(project.agent_workdir(&aid)).unwrap();
        let tab = Tab::new(
            Some(app.router.clone()),
            aid.clone(),
            AgentState::fake(),
            &project,
        );
        app.tabs.insert(aid.clone(), tab);
        app.rebuild_tablist();
        app.select_tab(Some(0));
        let (tx, _rx) = channel(8);
        let (user_tx, _user_rx) = channel(8);
        let (abort, _reg) = AbortHandle::new_pair();
        app.router.register_root(aid.clone()).await.unwrap();
        app.router
            .attach_runtime(aid.clone(), RuntimeHandle::new(tx, user_tx, abort))
            .await
            .unwrap();

        app.archive_tab().await.unwrap();

        assert!(app.tabs.is_empty());
        // workdir survives until `vc cleanup -f`
        assert!(project.agent(&aid).exists());
        // runtime is gone
        assert!(app.router.shutdown(aid.clone()).await.is_err());

        std::fs::remove_dir_all(project.agent(&aid)).ok();
    }

    #[tokio::test]
    async fn rejected_duplicate_rolls_back_preview() {
        let project = crate::project::Project::new_test().unwrap().0;
        let mut app = App::new(project.clone(), Default::default(), Default::default());
        let original = AgentId::from("original".to_string());
        app.tabs.insert(
            original.clone(),
            Tab::new(
                Some(app.router.clone()),
                original.clone(),
                AgentState::fake(),
                &project,
            ),
        );
        app.rebuild_tablist();
        app.select_tab(Some(0));

        assert!(app.duplicate_tab().await.is_err());
        assert_eq!(app.tabs.len(), 2);
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), app.rx.recv())
            .await
            .unwrap()
            .unwrap();
        app.handle(event).await.unwrap();

        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs.contains_key(&original));
    }

    #[tokio::test]
    async fn tab_selection_can_be_cleared_and_restored() {
        let mut app = App::new(
            crate::project::Project::new_test().unwrap().0,
            Default::default(),
            Default::default(),
        );
        let project = app.project.clone();
        let state = AgentState::fake();
        app.tabs = ["a", "b"]
            .into_iter()
            .map(|id| {
                let aid = AgentId::from(id.to_string());
                (aid.clone(), Tab::new(None, aid, state.clone(), &project))
            })
            .collect();
        app.rebuild_tablist();
        app.select_tab(Some(0));

        assert_eq!(app.selected_tab_idx(), Some(0));
        assert_eq!(app.select_tab(None), None);
        assert_eq!(app.selected_tab_idx(), None);
        assert_eq!(app.select_tab(Some(1)), Some(1));
        assert_eq!(app.selected_tab_idx(), Some(1));
    }
}

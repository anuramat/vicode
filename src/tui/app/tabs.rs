use anyhow::Result;
use git2::Repository;
use indexmap::IndexMap;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentState;
use crate::agent::handle::ExternalEvent;
use crate::agent::id::AgentId;
use crate::project::layout::LayoutTrait;
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
        agents: Vec<(AgentId, AgentState)>,
    ) -> Result<()> {
        let mut tabs = IndexMap::new();
        for (aid, state) in &agents {
            tabs.insert(
                aid.clone(),
                Tab::new(None, aid.clone(), state.clone(), &self.project),
            );
        }
        self.tabs = tabs;
        self.rebuild_tablist();

        for (aid, state) in agents {
            let agent = Agent::new(
                self.project.clone(),
                self.router.clone(),
                aid.clone(),
                state,
            );
            let runtime = agent.spawn();
            self.router.register(aid, runtime).await?;
        }
        Ok(())
    }

    /// create a new primary agent, and a corresponding tab
    pub async fn new_tab(&mut self) -> Result<()> {
        let aid = self.router.allocate_agent_id().await?;
        let repo = Repository::discover(self.project.root())?;
        let commit = repo.head()?.peel_to_commit()?.id().to_string();
        let instructions = self.project.instructions(&aid).await?;
        let assistant = self.project.assistants().next_primary()?;
        let state = AgentState::new(
            assistant,
            commit,
            instructions,
            self.project.config().subagent_max_depth,
        );
        self.insert_preview_tab(aid.clone(), state.clone());
        self.tx
            .send(AppEvent::NewAgent(aid, Box::new(state)))
            .await?;
        Ok(())
    }

    fn insert_preview_tab(
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
            .new_agent_workdir(&state.context.commit, &aid, true)
            .await?;
        state.save(&self.project, &aid).await?;
        let agent = Agent::new(
            self.project.clone(),
            self.router.clone(),
            aid.clone(),
            state,
        );
        let runtime = agent.spawn();
        self.router.register(aid, runtime).await?;
        Ok(())
    }

    pub async fn handle_started(
        &mut self,
        aid: &AgentId,
        state: AgentState,
    ) -> Result<()> {
        let router = self.router.clone();
        let tab = self.tab_mut_by_aid(aid)?;
        tab.state = state;
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
        self.insert_preview_tab(aid.clone(), state);

        router
            .forward(original_aid, ExternalEvent::DuplicateRequest(aid))
            .await?;
        Ok(())
    }

    /// archive selected tab: the agent stops running and disappears from the
    /// UI, but its state row and workdir stay until `vc cleanup -f`
    pub async fn archive_tab(&mut self) -> Result<()> {
        let idx = self
            .selected_tab_idx()
            .ok_or_else(|| anyhow::anyhow!("no tab selected"))?;
        let (_, tab) = self
            .tabs
            .get_index(idx)
            .ok_or_else(|| anyhow::anyhow!("tab with idx {idx} not found"))?;
        let router = tab.router()?.clone();
        let (aid, _) = self
            .tabs
            .shift_remove_index(idx)
            .ok_or_else(|| anyhow::anyhow!("tab with idx {idx} not found"))?;
        router.delete(aid.clone()).await?;
        self.project.unmount_agent(&aid).await?;
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
    use crate::agent::AgentStatus;
    use crate::llm::history::History;
    use crate::llm::provider::assistant::Assistant;

    fn state() -> AgentState {
        AgentState {
            status: AgentStatus::default(),
            assistant: Assistant::fake().0,
            max_depth: 1,
            context: crate::agent::AgentContext {
                commit: "".into(),
                history: History::new("".into()),
            },
        }
    }

    #[tokio::test]
    async fn new_tab_enqueues_agent_creation() {
        let mut app = App::new(
            crate::project::Project::new_test().unwrap(),
            Default::default(),
        );

        app.new_tab().await.unwrap();

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.selected_tab_idx(), Some(0));
        let (tab_aid, tab) = app.tabs.get_index(0).unwrap();
        assert!(tab.router.is_none());
        assert!(!tab.state.context.commit.is_empty());
        let instructions = app.project.instructions(tab_aid).await.unwrap();
        assert_eq!(tab.state.context.history.instructions(), instructions);

        match app.rx.recv().await {
            Some(AppEvent::NewAgent(aid, state)) => {
                assert_eq!(&aid, tab_aid);
                assert_eq!(state.context.commit, tab.state.context.commit);
                assert_eq!(
                    state.context.history.instructions(),
                    tab.state.context.history.instructions()
                );
            }
            other => panic!("expected NewAgent, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn archive_tab_stops_agent_but_keeps_workdir() {
        use futures::future::AbortHandle;
        use tokio::sync::mpsc::channel;

        use crate::agent::router::RuntimeHandle;

        let project = crate::project::Project::new_test().unwrap();
        let mut app = App::new(project.clone(), Default::default());
        let aid = AgentId::from(format!("archive-me-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(project.agent_workdir(&aid)).unwrap();
        let tab = Tab::new(Some(app.router.clone()), aid.clone(), state(), &project);
        app.tabs.insert(aid.clone(), tab);
        app.rebuild_tablist();
        app.select_tab(Some(0));
        let (tx, _rx) = channel(8);
        let (abort, _reg) = AbortHandle::new_pair();
        app.router
            .register(aid.clone(), RuntimeHandle::new(tx, abort))
            .await
            .unwrap();

        app.archive_tab().await.unwrap();

        assert!(app.tabs.is_empty());
        // workdir survives until `vc cleanup -f`
        assert!(project.agent(&aid).exists());
        // runtime is gone
        assert!(app.router.delete(aid.clone()).await.is_err());

        std::fs::remove_dir_all(project.agent(&aid)).ok();
    }

    #[tokio::test]
    async fn tab_selection_can_be_cleared_and_restored() {
        let mut app = App::new(
            crate::project::Project::new_test().unwrap(),
            Default::default(),
        );
        let state = state();
        let project = app.project.clone();
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

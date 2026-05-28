use anyhow::Result;
use git2::Repository;
use indexmap::IndexMap;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentState;
use crate::agent::handle::ExternalEvent;
use crate::agent::id::AgentId;
use crate::git::is_workdir_clean;
use crate::project::layout::LayoutTrait;
use crate::tui::app::App;
use crate::tui::app::AppEvent;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::Tab;
use crate::tui::tab::TabEntry;

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
        // TODO get rid of Loading, just insert tabs here
        let mut tabs = IndexMap::new();
        for (aid, _) in &agents {
            tabs.insert(aid.clone(), TabEntry::Loading);
        }
        self.tabs = tabs;
        self.rebuild_tablist();

        for (aid, agent_state) in agents {
            self.load_agent(aid, agent_state).await?;
        }
        Ok(())
    }

    /// create a new primary agent, and a corresponding tab
    pub async fn new_tab(&mut self) -> Result<()> {
        let aid = self.router.allocate_agent_id().await?;
        self.insert_loading_tab(aid.clone());
        self.tx.send(AppEvent::NewAgent(aid)).await?;
        Ok(())
    }

    /// inserts and selects a dummy tab
    fn insert_loading_tab(
        &mut self,
        aid: AgentId,
    ) {
        let idx = self.selected_tab_idx().map_or(self.tabs.len(), |x| x + 1);
        self.tabs.shift_insert(idx, aid, TabEntry::Loading);
        self.select_tab(Some(idx));
        self.rebuild_tablist();
    }

    pub async fn load_agent(
        &self,
        aid: AgentId,
        state: AgentState,
    ) -> Result<()> {
        let agent = Agent::from_state(
            self.project.clone(),
            self.router.clone(),
            aid.clone(),
            state,
        );
        let runtime = agent.spawn();
        self.router.register(aid, runtime).await?;
        Ok(())
    }

    pub async fn new_agent(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        let repo = Repository::discover(self.project.root())?;
        let commit = repo.head()?.peel_to_commit()?.id().to_string();
        let instructions = self.project.instructions(&aid).await?;
        let agent = Agent::new(
            self.project.clone(),
            self.router.clone(),
            aid.clone(),
            commit,
            instructions,
        )
        .await?;
        let runtime = agent.spawn();
        self.router.register(aid, runtime).await?;
        Ok(())
    }

    pub fn handle_started(
        &mut self,
        aid: &AgentId,
        state: AgentState,
    ) -> Result<()> {
        // ignore subagents (no Loading slot reserved)
        let Some(entry) = self.tabs.get_mut(aid) else {
            return Ok(());
        };
        if !matches!(entry, TabEntry::Loading) {
            return Ok(());
        }
        let tab = Tab::new(self.router.clone(), aid.clone(), state, &self.project)?;
        *entry = TabEntry::Ready(tab);
        self.rebuild_tablist();
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn duplicate_tab(&mut self) -> Result<()> {
        let original_aid = self.selected_tab()?.aid.clone();

        let aid = self.router.allocate_agent_id().await?;
        self.insert_loading_tab(aid.clone());

        self.router
            .forward(original_aid, ExternalEvent::DuplicateRequest(aid))
            .await?;
        Ok(())
    }

    /// delete selected tab and corresponding agent
    pub async fn delete_tab(&mut self) -> Result<()> {
        let Some(idx) = self.selected_tab_idx() else {
            return Ok(());
        };
        let Some((aid, TabEntry::Ready(_))) = self.tabs.get_index(idx) else {
            return Ok(());
        };
        anyhow::ensure!(
            is_workdir_clean(&self.project.agent_workdir(aid))?,
            "workdir has uncommitted changes"
        );
        let Some((aid, TabEntry::Ready(tab))) = self.tabs.shift_remove_index(idx) else {
            return Ok(());
        };
        let commit = tab.state.context.commit.clone();
        self.router.delete(aid.clone()).await?;
        self.project.delete_agent(&aid, &commit).await?;
        self.rebuild_tablist();
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
        self.tab_by_idx(idx)
    }

    pub fn tab_mut_by_aid(
        &mut self,
        aid: &AgentId,
    ) -> Result<&mut Tab<'a>> {
        let Some(entry) = self.tabs.get_mut(aid) else {
            anyhow::bail!("tab not found");
        };
        match entry {
            TabEntry::Loading => anyhow::bail!("tab is loading"),
            TabEntry::Ready(tab) => Ok(tab),
        }
    }

    // TODO maybe make a macro for getters?

    pub fn tab_mut_by_idx(
        &mut self,
        idx: usize,
    ) -> Result<&mut Tab<'a>> {
        let Some((_, entry)) = self.tabs.get_index_mut(idx) else {
            anyhow::bail!("tab not found");
        };
        match entry {
            TabEntry::Loading => anyhow::bail!("tab is loading"),
            TabEntry::Ready(tab) => Ok(tab),
        }
    }

    pub fn tab_by_idx(
        &self,
        idx: usize,
    ) -> Result<&Tab<'a>> {
        let Some((_, entry)) = self.tabs.get_index(idx) else {
            anyhow::bail!("tab not found");
        };
        match entry {
            TabEntry::Loading => anyhow::bail!("tab is loading"),
            TabEntry::Ready(tab) => Ok(tab),
        }
    }

    pub fn selected_tab_mut(&mut self) -> Result<&mut Tab<'a>> {
        let Some(idx) = self.selected_tab_idx() else {
            anyhow::bail!("no tab selected");
        };
        self.tab_mut_by_idx(idx)
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
                    tab.set_osc7(&self.project);
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
    async fn new_tab_enqueues_agent_creation() {
        let mut app = App::new(
            crate::project::Project::new_test().unwrap(),
            Default::default(),
        );

        app.new_tab().await.unwrap();

        match app.rx.recv().await {
            Some(AppEvent::NewAgent(_)) => {}
            other => panic!("expected NewAgent, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tab_selection_can_be_cleared_and_restored() {
        let mut app = App::new(
            crate::project::Project::new_test().unwrap(),
            Default::default(),
        );
        app.tabs = ["a", "b"]
            .into_iter()
            .map(|id| (AgentId::from(id.to_string()), TabEntry::Loading))
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

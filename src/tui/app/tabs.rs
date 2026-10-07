use anyhow::Result;
use git2::Repository;
use indexmap::IndexMap;
use tokio::sync::mpsc::UnboundedSender;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentState;
use crate::agent::event::UserCommand;
use crate::agent::id::AgentId;
use crate::agent::router::Launch;
use crate::tui::app::App;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::Tab;

impl<'a> App<'a> {
    /// rebuild tablist widget
    pub fn rebuild_tablist(&mut self) {
        self.tablist.rebuild(&self.tabs);
        self.select_tab(self.selected_tab_idx());
    }

    pub fn load_tabs(
        &mut self,
        tab_agents: Vec<(AgentId, AgentState)>,
        agents: Vec<Launch>,
    ) {
        let mut tabs = IndexMap::new();
        for (aid, state) in &tab_agents {
            tabs.insert(
                aid.clone(),
                Tab::new(None, aid.clone(), state.clone(), &self.project),
            );
        }
        self.tabs = tabs;
        self.rebuild_tablist();

        // every node is live already: launch order doesn't matter
        for launch in agents {
            launch.go();
        }
    }

    /// create a new primary agent, and a corresponding tab
    pub async fn new_tab(&mut self) -> Result<()> {
        let aid = self.router.allocate_agent_id();
        let repo = Repository::discover(self.project.root())?;
        let commit = repo.head()?.peel_to_commit()?.id().to_string();
        // context files are read from the agent's own tree
        self.project.new_agent_workdir(&commit, &aid).await?;
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
        let agent = Agent::new(
            self.project.clone(),
            self.router.clone(),
            self.tx.clone(),
            aid,
            state,
        );
        agent.launch_root().await
    }

    pub async fn handle_started(
        &mut self,
        aid: &AgentId,
        state: AgentState,
        control: UnboundedSender<UserCommand>,
    ) -> Result<()> {
        let tab = self.tab_mut_by_aid(aid)?;
        tab.state = state;
        // a (re)start's fresh runtime has no in-flight calls; the prior
        // runtime's tee'd output is stale render state
        tab.live_output.clear();
        tab.refresh_assistant_config();
        tab.control = Some(control);
        self.rebuild_tablist();
        self.refresh();
        self.save_app_state().await?;
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn duplicate_tab(&mut self) -> Result<()> {
        let original = self.selected_tab()?;
        if original.control.is_none() {
            return Ok(());
        }
        let state = original.state.clone();
        let copy = self.router.allocate_agent_id();
        original.send(UserCommand::Duplicate(copy.clone()))?;
        // a preview until the copy's Started attaches it; the original
        // reports a failure as DuplicateFailed, which drops it again
        self.insert_tab(copy, state);
        Ok(())
    }

    /// archive selected tab: every member goes `Unreachable`, graph records
    /// flip to `archived`, mounts release — state + workdirs retained
    pub async fn archive_tab(&mut self) -> Result<()> {
        let idx = self
            .selected_tab_idx()
            .ok_or_else(|| anyhow::anyhow!("no tab selected"))?;
        let (_, tab) = self
            .tabs
            .get_index(idx)
            .ok_or_else(|| anyhow::anyhow!("tab with idx {idx} not found"))?;
        anyhow::ensure!(tab.control.is_some(), "agent isn't attached (yet?)");
        let (aid, _) = self
            .tabs
            .shift_remove_index(idx)
            .ok_or_else(|| anyhow::anyhow!("tab with idx {idx} not found"))?;
        self.router.archive_tab(&aid).await?;
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
        let mut app = App::new(crate::project::Project::new_test().unwrap().0);

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
        app.router.shutdown(&tab_aid).unwrap();
    }

    #[tokio::test]
    async fn archive_tab_stops_agent_but_keeps_workdir() {
        let project = crate::project::Project::new_test().unwrap().0;
        let mut app = App::new(project.clone());
        let aid = AgentId::from(format!("archive-me-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(project.agent_workdir(&aid)).unwrap();
        let tab = Tab::new(
            Some(tokio::sync::mpsc::unbounded_channel().0),
            aid.clone(),
            AgentState::fake(),
            &project,
        );
        app.tabs.insert(aid.clone(), tab);
        app.rebuild_tablist();
        app.select_tab(Some(0));
        app.router
            .attach_manual(&aid, tokio::sync::mpsc::unbounded_channel().0);

        app.archive_tab().await.unwrap();

        assert!(app.tabs.is_empty());
        // workdir survives until `vc cleanup -f`
        assert!(project.agent(&aid).exists());
        // runtime is gone
        assert!(app.router.shutdown(&aid).is_err());

        std::fs::remove_dir_all(project.agent(&aid)).ok();
    }

    #[tokio::test]
    async fn undeliverable_duplicate_adds_no_preview() {
        let project = crate::project::Project::new_test().unwrap().0;
        let mut app = App::new(project.clone());
        let original = AgentId::from("original".to_string());
        app.tabs.insert(
            original.clone(),
            Tab::new(
                Some(tokio::sync::mpsc::unbounded_channel().0),
                original.clone(),
                AgentState::fake(),
                &project,
            ),
        );
        app.rebuild_tablist();
        app.select_tab(Some(0));

        // the original has no runtime: the request is rejected before any
        // preview exists
        assert!(app.duplicate_tab().await.is_err());
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs.contains_key(&original));
    }

    #[tokio::test]
    async fn tab_selection_can_be_cleared_and_restored() {
        let mut app = App::new(crate::project::Project::new_test().unwrap().0);
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

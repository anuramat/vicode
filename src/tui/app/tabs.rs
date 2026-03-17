use anyhow::Result;
use git2::Repository;
use indexmap::IndexMap;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentEvent;
use crate::agent::AgentId;
use crate::agent::handle::ParentEvent;
use crate::llm::history::History;
use crate::project::PROJECT;
use crate::tui::app::App;
use crate::tui::app::AppEvent;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::Tab;
use crate::tui::tab::TabState;

impl<'a> App<'a> {
    /// rebuild tablist widget
    pub fn rebuild_tablist(&mut self) {
        self.tablist.rebuild(&self.tabs);
        // make sure index is in bounds after rebuild
        self.select_tab(self.selected_tab());
    }

    pub async fn load_tabs(&mut self) -> Result<()> {
        let state = PROJECT
            .load_app_state()
            .await
            .expect("failed to load app state");
        let tabs: IndexMap<AgentId, Tab<'_>> = state
            .primary_agents
            .iter()
            .map(|aid| (aid.clone(), Tab::loading_tab(self.tx.clone(), aid.clone())))
            .collect();
        self.tabs = tabs;
        self.rebuild_tablist();

        for aid in state.primary_agents {
            self.tx.send(AppEvent::AttachAgent(aid.clone())).await?
        }
        Ok(())
    }

    /// create a new primary agent, and a corresponding tab
    pub async fn new_tab(&mut self) -> Result<()> {
        let id = AgentId::new();
        self.insert_tab(id.clone(), Default::default()).await?;
        self.tx.send(AppEvent::AttachAgent(id)).await?;
        Ok(())
    }

    async fn insert_tab(
        &mut self,
        id: AgentId,
        history: History,
    ) -> Result<()> {
        let tab = Tab::new(self.tx.clone(), id.clone(), history).await?;
        let idx = self
            .selected_tab()
            .map(|x| x + 1)
            .unwrap_or(self.tabs.len());
        self.tabs.shift_insert(idx, id.clone(), tab);
        self.select_tab(Some(idx));
        self.rebuild_tablist();
        Ok(())
    }

    pub async fn attach_agent(
        &mut self,
        aid: AgentId,
    ) -> Result<()> {
        anyhow::ensure!(
            self.tabs.contains_key(&aid),
            "tab for agent {:?} not found",
            &aid
        );

        let agent: Agent = if PROJECT.agent(&aid).exists() {
            Agent::load(self.parent_tx.clone(), aid.clone()).await?
        } else {
            let repo = Repository::discover(PROJECT.root.clone())?;
            let commit = repo.head()?.peel_to_commit()?.id().to_string();
            let instructions = PROJECT.instructions_by_commit(&commit).await?;
            Agent::new(self.parent_tx.clone(), aid.clone(), commit, instructions).await?
        };

        let tab = Tab::new(
            self.tx.clone(),
            aid.clone(),
            agent.state.context.history.clone(),
        )
        .await?;
        self.tabs.insert(agent.id.clone(), tab);
        self.agents.insert(agent.id.clone(), agent.tx.clone());
        self.joinset.spawn(agent.run());
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn duplicate_tab(&mut self) -> Result<()> {
        let (history, tx) = if let Some((aid, tab)) =
            self.selected_tab().and_then(|idx| self.tabs.get_index(idx))
            && let Some(tx) = self.agents.get(aid)
        {
            if !matches!(tab.state, TabState::Idle) {
                return Ok(());
            }
            (tab.history.data.clone(), tx.clone())
        } else {
            return Ok(());
        };
        let aid = AgentId::new();
        self.insert_tab(aid.clone(), history).await?;
        tx.send(AgentEvent::DuplicateRequest(aid.clone())).await?;
        Ok(())
    }

    /// delete selected tab and corresponding agent
    pub async fn delete_tab(&mut self) -> Result<()> {
        if let Some(idx) = self.selected_tab() {
            // delete tab
            if let Some((_, tab)) = self.tabs.shift_remove_index(idx) {
                let aid = tab.aid;
                if let Some(tx) = self.agents.remove(&aid) {
                    // delete agent
                    tx.send(AgentEvent::Delete).await?;
                }
            };
        } else {
            return Ok(());
        };

        self.rebuild_tablist();

        Ok(())
    }

    pub fn selected_tab(&self) -> Option<usize> {
        let n_tabs = self.tabs.len();
        if n_tabs == 0 {
            return None;
        };
        self.tablist.selected().map(|s| s.min(n_tabs - 1))
    }

    pub fn next_tab(&mut self) {
        let Some(idx) = self.selected_tab() else {
            self.select_tab(Some(0));
            return;
        };
        self.select_tab(Some(idx.checked_add(1).expect("tab index overflow")));
    }

    pub fn prev_tab(&mut self) {
        let Some(idx) = self.selected_tab() else {
            self.last_tab();
            return;
        };
        self.select_tab(idx.checked_sub(1));
    }

    /// reset OSC7 to the project root
    pub fn reset_osc7(&self) {
        set_osc7(&PROJECT.root);
    }

    /// select a tab, checking the index
    fn select_tab(
        &mut self,
        mut idx: Option<usize>,
    ) -> Option<usize> {
        idx = idx.and_then(|i| {
            let n_tabs = self.tabs.len();
            if n_tabs == 0 || i >= n_tabs {
                self.reset_osc7();
                None
            } else {
                if let Some((_, tab)) = self.tabs.get_index(i) {
                    tab.set_osc7();
                };
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

// TODO move
pub async fn translate_agent_events(
    app_tx: Sender<AppEvent>,
    mut parent_rx: Receiver<ParentEvent>,
) -> Result<()> {
    while let Some(event) = parent_rx.recv().await {
        use ParentEvent::*;
        match event {
            InfoUpdate(aid) => app_tx.send(AppEvent::InfoUpdate(aid)).await?,
            HistoryUpdate(aid, history_event) => {
                app_tx
                    .send(AppEvent::HistoryUpdate(aid, history_event))
                    .await?
            }
            AttachAgent(aid) => app_tx.send(AppEvent::AttachAgent(aid)).await?,
            TurnComplete(aid) => {
                app_tx.send(AppEvent::AgentIdle(aid)).await?;
            }
        }
    }
    Ok(())
}

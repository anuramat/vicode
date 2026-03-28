use anyhow::Result;
use git2::Repository;
use indexmap::IndexMap;
use tokio::sync::mpsc::Sender;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentEvent;
use crate::agent::AgentState;
use crate::agent::handle::ParentEvent;
use crate::agent::handle::ParentSink;
use crate::agent::id::AgentId;
use crate::project::PROJECT;
use crate::tui::app::App;
use crate::tui::app::AppEvent;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::Tab;

struct AppParentSink {
    aid: AgentId,
    tx: Sender<AppEvent>,
}

#[async_trait::async_trait]
impl ParentSink for AppParentSink {
    async fn send(
        &self,
        event: ParentEvent,
    ) -> Result<()> {
        let aid = self.aid.clone();
        self.tx
            .send(AppEvent::ParentEvent(aid, event))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

impl<'a> App<'a> {
    /// rebuild tablist widget
    pub fn rebuild_tablist(&mut self) {
        self.tablist.rebuild(&self.tabs);
        // make sure index is in bounds after rebuild
        self.select_tab(self.selected_tab_idx());
    }

    pub async fn load_tabs(&mut self) -> Result<()> {
        let state = PROJECT
            .load_app_state()
            .await
            .expect("failed to load app state");
        let mut tabs = IndexMap::new();
        for aid in &state.primary_agents {
            let agent_state = PROJECT.load_agent_state(aid).await?;
            tabs.insert(
                aid.clone(),
                Tab::loading_tab(self.tx.clone(), aid.clone(), agent_state),
            );
        }
        self.tabs = tabs;
        self.rebuild_tablist();

        for aid in state.primary_agents {
            self.tx
                .send(AppEvent::ParentEvent(aid.clone(), ParentEvent::AttachAgent))
                .await?
        }
        Ok(())
    }

    /// create a new primary agent, and a corresponding tab
    pub async fn new_tab(&mut self) -> Result<()> {
        let id = AgentId::new().await?;
        self.insert_tab(id.clone(), Default::default()).await?;
        self.tx
            .send(AppEvent::ParentEvent(id, ParentEvent::AttachAgent))
            .await?;
        Ok(())
    }

    async fn insert_tab(
        &mut self,
        id: AgentId,
        agent_state: AgentState,
    ) -> Result<()> {
        let tab = Tab::loading_tab(self.tx.clone(), id.clone(), agent_state);
        let idx = self
            .selected_tab_idx()
            .map(|x| x + 1)
            .unwrap_or(self.tabs.len());
        self.tabs.shift_insert(idx, id, tab);
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
            Agent::load(
                Box::new(AppParentSink {
                    aid: aid.clone(),
                    tx: self.tx.clone(),
                }),
                aid.clone(),
            )
            .await?
        } else {
            let repo = Repository::discover(PROJECT.root.clone())?;
            let commit = repo.head()?.peel_to_commit()?.id().to_string();
            let instructions = PROJECT.instructions_by_commit(&commit).await?;
            Agent::new(
                Box::new(AppParentSink {
                    aid: aid.clone(),
                    tx: self.tx.clone(),
                }),
                aid.clone(),
                commit,
                instructions,
            )
            .await?
        };

        let tab = Tab::new(self.tx.clone(), aid.clone(), agent.state.clone()).await?;
        self.tabs.insert(agent.id.clone(), tab);
        self.agents.insert(agent.id.clone(), agent.tx.clone());
        self.joinset.spawn(agent.run());
        self.rebuild_tablist();
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn duplicate_tab(&mut self) -> Result<()> {
        let (agent_state, tx) = if let Some((aid, tab)) = self
            .selected_tab_idx()
            .and_then(|idx| self.tabs.get_index(idx))
            && let Some(tx) = self.agents.get(aid)
        {
            if !tab.state.idle() {
                return Ok(());
            }
            (tab.agent_state.clone(), tx.clone())
        } else {
            return Ok(());
        };
        let aid = AgentId::new().await?;
        self.insert_tab(aid.clone(), agent_state).await?;
        tx.send(AgentEvent::DuplicateRequest(aid.clone())).await?;
        Ok(())
    }

    /// delete selected tab and corresponding agent
    pub async fn delete_tab(&mut self) -> Result<()> {
        if let Some(idx) = self.selected_tab_idx() {
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

    // TODO go through calls and use selected_tab()/selected_tab_mut() instead; maybe make this private afterwards; maybe make full getters
    pub fn selected_tab_idx(&self) -> Option<usize> {
        let n_tabs = self.tabs.len();
        if n_tabs == 0 {
            return None;
        };
        self.tablist.selected().map(|s| s.min(n_tabs - 1))
    }

    pub fn selected_tab(&self) -> Result<&Tab<'a>> {
        let Some(idx) = self.selected_tab_idx() else {
            anyhow::bail!("no tab selected");
        };
        let Some((_, tab)) = self.tabs.get_index(idx) else {
            anyhow::bail!("selected tab not found");
        };
        Ok(tab)
    }

    pub fn selected_tab_mut(&mut self) -> Result<&mut Tab<'a>> {
        let Some(idx) = self.selected_tab_idx() else {
            anyhow::bail!("no tab selected");
        };
        let Some((_, tab)) = self.tabs.get_index_mut(idx) else {
            anyhow::bail!("selected tab not found");
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
                set_osc7(&PROJECT.root);
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

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::config::CONFIG;

    #[tokio::test]
    async fn tab_selection_can_be_cleared_and_restored() {
        let mut app = App::new().await.unwrap();
        let (tx, _) = channel(1);
        let assistant_id = CONFIG.assistants.keys().next().unwrap().clone();
        app.tabs = ["a", "b"]
            .into_iter()
            .map(|id| {
                let aid = AgentId::from(id.to_string());
                let mut agent_state = AgentState::default();
                agent_state.context.assistant_id = assistant_id.clone();
                (aid.clone(), Tab::loading_tab(tx.clone(), aid, agent_state))
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

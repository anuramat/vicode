use anyhow::Context;
use anyhow::Result;
use git2::Repository;
use indexmap::IndexMap;
use tokio::sync::mpsc::Sender;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentEvent;
use crate::agent::handle::AgentStarted;
use crate::agent::handle::ParentEvent;
use crate::agent::handle::ParentHandle;
use crate::agent::handle::ParentSink;
use crate::agent::id::AgentId;
use crate::project::PROJECT;
use crate::tui::app::AgentHandle;
use crate::tui::app::App;
use crate::tui::app::AppEvent;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::Tab;
use crate::tui::tab::TabEntry;

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
        self.tx
            .send(AppEvent::ParentEvent(self.aid.clone(), event))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    fn sibling(
        &self,
        aid: AgentId,
    ) -> ParentHandle {
        Box::new(Self {
            aid,
            tx: self.tx.clone(),
        })
    }
}

impl<'a> App<'a> {
    /// rebuild tablist widget
    pub fn rebuild_tablist(&mut self) {
        self.tablist.rebuild(&self.tabs);
        self.select_tab(self.selected_tab_idx());
    }

    pub async fn load_tabs(&mut self) -> Result<()> {
        let state = PROJECT
            .load_app_state()
            .await
            .expect("failed to load app state");
        let mut tabs = IndexMap::new();
        for aid in &state.primary_agents {
            tabs.insert(aid.clone(), TabEntry::Loading);
        }
        self.tabs = tabs;
        self.rebuild_tablist();

        for aid in state.primary_agents {
            self.tx.send(AppEvent::LoadAgent(aid)).await?;
        }
        Ok(())
    }

    /// create a new primary agent, and a corresponding tab
    pub async fn new_tab(&mut self) -> Result<()> {
        let aid = AgentId::new().await?;
        self.insert_loading_tab(aid.clone());
        self.tx.send(AppEvent::LoadAgent(aid)).await?;
        Ok(())
    }

    fn insert_loading_tab(
        &mut self,
        aid: AgentId,
    ) {
        let idx = self
            .selected_tab_idx()
            .map(|x| x + 1)
            .unwrap_or(self.tabs.len());
        self.tabs.shift_insert(idx, aid, TabEntry::Loading);
        self.select_tab(Some(idx));
        self.rebuild_tablist();
    }

    pub async fn load_agent(
        &mut self,
        aid: AgentId,
    ) -> Result<()> {
        let parent = Box::new(AppParentSink {
            aid: aid.clone(),
            tx: self.tx.clone(),
        });
        let agent = if PROJECT.agent(&aid).exists() {
            Agent::load(parent, aid).await?
        } else {
            let repo = Repository::discover(PROJECT.root.clone())?;
            let commit = repo.head()?.peel_to_commit()?.id().to_string();
            let instructions = PROJECT.instructions_by_commit(&commit).await?;
            Agent::new(parent, aid, commit, instructions).await?
        };
        agent.spawn();
        Ok(())
    }

    pub async fn handle_started(
        &mut self,
        started: AgentStarted,
    ) -> Result<()> {
        let tab = Tab::new(self.tx.clone(), started.aid.clone(), started.state).await?;
        self.tabs.insert(started.aid.clone(), TabEntry::Ready(tab));
        self.agents.insert(started.aid.clone(), started.handle);
        self.rebuild_tablist();
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn duplicate_tab(&mut self) -> Result<()> {
        // TODO this is kinda ugly
        let tab = self.selected_tab()?;
        if !tab.state.idle() {
            return Ok(());
        }
        let tx = self
            .agents
            .get(&tab.aid)
            .with_context(|| format!("agent handle for {} not found", tab.aid))?
            .tx
            .clone();
        let new_aid = AgentId::new().await?;
        self.insert_loading_tab(new_aid.clone());
        tx.send(AgentEvent::DuplicateRequest(new_aid)).await?;
        Ok(())
    }

    /// delete selected tab and corresponding agent
    pub async fn delete_tab(&mut self) -> Result<()> {
        if let Some(idx) = self.selected_tab_idx() {
            if let Some((aid, _)) = self.tabs.shift_remove_index(idx)
                && let Some(handle) = self.agents.remove(&aid)
                && handle.tx.send(AgentEvent::Delete).await.is_err()
            {
                handle.abort.abort();
            }
        } else {
            return Ok(());
        }

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
                set_osc7(&PROJECT.root);
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
    use super::*;

    #[tokio::test]
    async fn tab_selection_can_be_cleared_and_restored() {
        let mut app = App::new().await.unwrap();
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

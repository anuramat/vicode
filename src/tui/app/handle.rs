use anyhow::Result;
use crossterm::event::KeyEvent;
use tracing::debug;
use tracing::instrument;

use super::App;
use super::NotificationKind;
use crate::agent::AgentEvent;
use crate::agent::handle::ParentEvent as AgentParentEvent;
use crate::agent::handle::UserPrompt;
use crate::agent::id::AgentId;
use crate::llm::history::HistoryEvent;
use crate::llm::history::HistoryGeneration;
use crate::tui::tab::AssistantState;
use crate::tui::tab::TabState;
use crate::tui::widgets::info::InfoWidget;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Paste(String),

    // TODO combine these into a single variant?
    UserPrompt(AgentId, UserPrompt),
    RetryTurn(AgentId),
    HistoryEvent(AgentId, HistoryGeneration, HistoryEvent),
    SetAssistant(AgentId, String),

    LoadAgent(AgentId),
    ParentEvent(AgentId, AgentParentEvent),
    TabStatusChanged(AgentId),

    Redraw,
}

impl<'a> App<'a> {
    #[instrument(skip(self))]
    pub async fn handle(
        &mut self,
        event: AppEvent,
    ) -> Result<()> {
        use AppEvent::*;

        debug!(event = ?event, "Handling app event");
        match event {
            Key(key_event) => {
                self.key(key_event).await?;
            }
            Paste(content) => {
                self.selected_tab_mut()?.paste(&content).await;
                self.dirty = true;
            }
            UserPrompt(agent_id, msg) => {
                if let Some(handle) = self.agents.get(&agent_id) {
                    handle.tx.send(AgentEvent::Submit(msg)).await?;
                }
            }
            RetryTurn(agent_id) => {
                if let Some(handle) = self.agents.get(&agent_id) {
                    handle.tx.send(AgentEvent::Retry).await?;
                }
            }
            HistoryEvent(agent_id, generation, event) => {
                if let Some(handle) = self.agents.get(&agent_id) {
                    handle
                        .tx
                        .send(AgentEvent::HistoryEvent(generation, event))
                        .await?;
                }
            }
            SetAssistant(agent_id, id) => {
                if let Some(handle) = self.agents.get(&agent_id) {
                    handle.tx.send(AgentEvent::SetAssistant(id)).await?;
                }
            }
            LoadAgent(agent_id) => {
                self.load_agent(agent_id).await?;
            }
            ParentEvent(agent_id, event) => {
                self.handle_parent_event(agent_id, event).await?;
                self.dirty = true;
            }
            TabStatusChanged(agent_id) => {
                if self.tabs.contains_key(&agent_id) {
                    self.rebuild_tablist();
                }
                self.dirty = true;
            }
            Redraw => {
                self.dirty = true;
            }
        }
        Ok(())
    }

    async fn handle_parent_event(
        &mut self,
        aid: AgentId,
        event: AgentParentEvent,
    ) -> Result<()> {
        use AgentParentEvent::*;

        match event {
            Started(started) => {
                self.handle_started(started).await?;
            }
            InfoUpdate => {
                self.tab_mut_by_aid(&aid)?.info = InfoWidget::new(&aid).await?;
            }
            HistoryReset(history) => {
                self.tab_mut_by_aid(&aid)?.replace_history(history);
            }
            HistoryUpdate(loc, event) => {
                self.tab_mut_by_aid(&aid)?.update(loc, event);
            }
            TurnComplete => {
                self.tab_mut_by_aid(&aid)?.sync_state_from_history().await?;
            }
            Error(_) => {
                self.tab_mut_by_aid(&aid)?
                    .set_state(TabState::Running(AssistantState::Error))
                    .await?;
                // TODO use msg
            }
        }
        Ok(())
    }
}

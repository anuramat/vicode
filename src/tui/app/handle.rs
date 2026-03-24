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
use crate::llm::history::HistoryLoc;
use crate::tui::tab::AssistantState;
use crate::tui::tab::TabState;
use crate::tui::widgets::info::InfoWidget;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),

    UserPrompt(AgentId, UserPrompt),
    RetryTurn(AgentId),
    AbortTurn(HistoryLoc, AgentId),
    SetAssistant(AgentId, String),
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
                if let Err(err) = self.key(key_event).await {
                    self.notify(NotificationKind::Error, err.to_string());
                }
                self.dirty = true;
            }
            UserPrompt(agent_id, msg) => {
                if let Some(tx) = self.agents.get(&agent_id) {
                    tx.send(AgentEvent::Submit(msg)).await?;
                }
            }
            // TODO maybe create a new type for agent events that come from outside
            RetryTurn(agent_id) => {
                if let Some(tx) = self.agents.get(&agent_id) {
                    tx.send(AgentEvent::Retry).await?;
                }
            }
            AbortTurn(loc, agent_id) => {
                if let Some(tx) = self.agents.get(&agent_id) {
                    tx.send(AgentEvent::HistoryEvent(loc, HistoryEvent::ResponseAborted))
                        .await?;
                }
            }
            SetAssistant(agent_id, id) => {
                if let Some(tx) = self.agents.get(&agent_id) {
                    tx.send(AgentEvent::SetAssistant(id)).await?;
                }
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
        agent_id: AgentId,
        event: AgentParentEvent,
    ) -> Result<()> {
        use AgentParentEvent::*;

        match event {
            AttachAgent => {
                self.attach_agent(agent_id).await?;
            }
            InfoUpdate => {
                if let Some(tab) = self.tabs.get_mut(&agent_id) {
                    tab.info = InfoWidget::new(&agent_id).await?;
                }
            }
            HistoryReset(history) => {
                if let Some(tab) = self.tabs.get_mut(&agent_id) {
                    tab.replace_history(history);
                }
            }
            HistoryUpdate(loc, event) => {
                if let Some(tab) = self.tabs.get_mut(&agent_id) {
                    tab.update(loc, event);
                }
            }
            TurnComplete => {
                if let Some(tab) = self.tabs.get_mut(&agent_id) {
                    tab.sync_state_from_history().await?;
                }
            }
            Error(msg) => {
                if let Some(tab) = self.tabs.get_mut(&agent_id) {
                    tab.set_state(TabState::Running(AssistantState::Error))
                        .await?;
                }
                if self.selected_aid() == Some(agent_id) {
                    self.notify(NotificationKind::Error, msg);
                }
            }
        }
        Ok(())
    }
}

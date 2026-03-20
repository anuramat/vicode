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
use crate::tui::tab::TabState;
use crate::tui::widgets::info::InfoWidget;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),

    UserPrompt(AgentId, UserPrompt),
    SetAssistant(AgentId, String),
    ParentEvent(AgentId, AgentParentEvent),

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
                self.dirty = true;
            }
            UserPrompt(agent_id, msg) => {
                if let Some(tx) = self.agents.get(&agent_id) {
                    tx.send(AgentEvent::Submit(msg)).await?;
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
            HistoryUpdate(loc, event) => {
                if let Some(tab) = self.tabs.get_mut(&agent_id) {
                    tab.update(loc, event);
                }
            }
            TurnComplete => {
                if let Some(tab) = self.tabs.get_mut(&agent_id) {
                    tab.state = TabState::Idle;
                }
            }
            Error(msg) => {
                if self.selected_aid() == Some(agent_id) {
                    self.notify(NotificationKind::Error, msg);
                }
            }
        }
        Ok(())
    }
}

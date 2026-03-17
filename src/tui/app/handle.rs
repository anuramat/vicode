use anyhow::Result;
use crossterm::event::KeyEvent;
use tracing::debug;
use tracing::instrument;

use super::App;
use super::NotificationKind;
use crate::agent::AgentEvent;
use crate::agent::AgentId;
use crate::agent::handle::UserPrompt;
use crate::llm::history::HistoryEvent;
use crate::tui::tab::TabState;
use crate::tui::widgets::info::InfoWidget;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),

    UserPrompt(AgentId, UserPrompt),
    // TODO replace with ParentEvent, and write a special handler
    InfoUpdate(AgentId),
    HistoryUpdate(AgentId, HistoryEvent),
    AgentIdle(AgentId),
    Error(AgentId, String),

    AttachAgent(AgentId),

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
            HistoryUpdate(agent_id, history_event) => {
                if let Some(tab) = self.tabs.get_mut(&agent_id) {
                    tab.update(history_event);
                }
                self.dirty = true;
            }
            InfoUpdate(agent_id) => {
                if let Some(tab) = self.tabs.get_mut(&agent_id) {
                    tab.info = InfoWidget::new(&agent_id).await?;
                }
                self.dirty = true;
            }
            UserPrompt(agent_id, msg) => {
                if let Some(tx) = self.agents.get(&agent_id) {
                    tx.send(AgentEvent::Submit(msg)).await?;
                }
            }
            AttachAgent(agent_id) => {
                self.attach_agent(agent_id).await?;
            }
            AgentIdle(agent_id) => {
                if let Some(tab) = self.tabs.get_mut(&agent_id) {
                    tab.state = TabState::Idle;
                }
                self.dirty = true;
            }
            Error(agent_id, msg) => {
                if self.selected_aid() == Some(agent_id) {
                    self.notify(NotificationKind::Error, msg);
                    self.dirty = true;
                }
            }
            Redraw => {}
        }
        Ok(())
    }
}

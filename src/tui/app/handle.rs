use anyhow::Result;
use crossterm::event::KeyEvent;
use tracing::debug;
use tracing::instrument;

use super::App;
use crate::agent::handle::ParentEvent as AgentParentEvent;
use crate::agent::id::AgentId;
use crate::tui::app::NotificationKind;
use crate::tui::widgets::info::InfoWidget;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Paste(String),

    LoadAgent(AgentId),
    NewAgent(AgentId),
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
            LoadAgent(agent_id) => {
                self.load_agent(agent_id).await?;
            }
            NewAgent(agent_id) => {
                self.new_agent(agent_id).await?;
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
            Started(agent) => {
                self.handle_started(aid, *agent).await?;
            }
            HistoryReset(history) => {
                self.tab_mut_by_aid(&aid)?.replace_history(history);
            }
            HistoryUpdate(loc, event) => {
                self.tab_mut_by_aid(&aid)?.update(loc, event).await?;
            }
            Error(msg) => {
                self.notify(NotificationKind::Error, msg);
            }
            StatusUpdate(status) => {
                if self.tab_mut_by_aid(&aid)?.set_state(status).await? {
                    self.tab_mut_by_aid(&aid)?.info = InfoWidget::new(&aid).await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::NotificationKind;

    #[tokio::test]
    async fn parent_error_creates_notification() {
        let mut app = App::new().await.unwrap();

        app.handle_parent_event(
            AgentId::from("a".to_string()),
            AgentParentEvent::Error("oops".into()),
        )
        .await
        .unwrap();

        let notification = app.notification.expect("expected notification");
        assert!(matches!(notification.kind, NotificationKind::Error));
        assert_eq!(notification.msg, "oops");
    }
}

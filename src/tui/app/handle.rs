use anyhow::Result;
use tracing::debug;
use tracing::instrument;

use super::App;
use super::AppEvent;
use crate::agent::handle::ParentEvent;
use crate::agent::id::AgentId;
use crate::tui::app::NotificationKind;

impl App<'_> {
    #[instrument(skip(self))]
    pub async fn handle(
        &mut self,
        event: AppEvent,
    ) -> Result<()> {
        #[allow(clippy::enum_glob_use)]
        use AppEvent::*;

        debug!(event = ?event, "Handling app event");
        match event {
            Key(key_event) => {
                self.key(key_event).await?;
            }
            Paste(content) => {
                self.selected_tab_mut()?.paste(&content);
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
        event: ParentEvent,
    ) -> Result<()> {
        #[allow(clippy::enum_glob_use)]
        use ParentEvent::*;

        // events for agents without a tab (subagents) are dropped — the
        // router still owns the runtime, completion is delivered via oneshot.
        match event {
            Started(state) => {
                self.handle_started(&aid, *state)?;
                if let Ok(tab) = self.tab_mut_by_aid(&aid) {
                    tab.refresh_info().await?;
                }
            }
            HistoryUpdate(loc, event) => {
                if let Ok(tab) = self.tab_mut_by_aid(&aid) {
                    tab.update(loc, event)?;
                }
            }
            Error(msg) => {
                if self.tabs.contains_key(&aid) {
                    self.notify(NotificationKind::Error, msg);
                }
            }
            StatusUpdate(status) => {
                if let Ok(tab) = self.tab_mut_by_aid(&aid)
                    && tab.set_state(status).await?
                {
                    tab.refresh_info().await?;
                }
            }
            AssistantSet(assistant) => {
                if let Ok(tab) = self.tab_mut_by_aid(&aid) {
                    tab.state.assistant = assistant;
                    tab.refresh_info().await?;
                    self.tx.send(AppEvent::TabStatusChanged(aid)).await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::tui::app::NotificationKind;

    #[tokio::test]
    async fn parent_error_creates_notification() {
        let mut app = App::new(crate::project::Project::new_test().unwrap());

        app.handle_parent_event(
            AgentId::from("a".to_string()),
            ParentEvent::Error("oops".into()),
        )
        .await
        .unwrap();

        let notification = app.notification.expect("expected notification");
        assert!(matches!(notification.kind, NotificationKind::Error));
        assert_eq!(notification.msg, "oops");
    }
}

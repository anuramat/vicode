use anyhow::Result;
use tracing::debug;
use tracing::instrument;

use super::App;
use super::AppEvent;
use crate::agent::handle::ParentEvent;
use crate::agent::id::AgentId;
use crate::tui::app::NotificationKind;
use crate::tui::widgets::info::InfoWidget;

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

        match event {
            Started(agent) => {
                self.handle_started(aid.clone(), *agent)?;
                let project = self.project.clone();
                self.tab_mut_by_aid(&aid)?.refresh_info(&project).await?;
            }
            HistoryReset(history) => {
                self.tab_mut_by_aid(&aid)?.replace_history(history);
            }
            HistoryUpdate(loc, event) => {
                self.tab_mut_by_aid(&aid)?.update(loc, event)?;
                // TODO resync history if tab handler fails
            }
            Error(msg) => {
                self.notify(NotificationKind::Error, msg);
            }
            StatusUpdate(status) => {
                let project = self.project.clone();
                if self
                    .tab_mut_by_aid(&aid)?
                    .set_state(status, &project)
                    .await?
                {
                    self.tab_mut_by_aid(&aid)?.refresh_info(&project).await?;
                }
            }
            SubagentDone(out) => {
                anyhow::bail!("unexpected subagent completion: {out:?}")
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

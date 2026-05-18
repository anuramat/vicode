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
    use git2::Repository;
    use similar_asserts::assert_eq;

    use super::*;
    use crate::agent::AgentState;
    use crate::agent::AgentStatus;
    use crate::agent::AgentVisibility;
    use crate::config::Config;
    use crate::llm::history::History;
    use crate::llm::provider::assistant::Assistant;
    use crate::llm::provider::assistant::AssistantPool;
    use crate::project::layout::LayoutTrait;
    use crate::tui::app::NotificationKind;
    use crate::tui::tab::Tab;
    use crate::tui::tab::TabEntry;

    async fn test_assistant() -> Assistant {
        crate::llm::provider::assistant::ASSISTANT_POOL
            .get_or_init(|| async {
                AssistantPool::from_config(
                    &Config::parse_with_defaults(
                        r#"
                primary_assistant = ["test"]
                shell_cmd = ["bash", "-c"]

                [sandbox]
                kind = "bwrap"
                bin = "bwrap"
                args = []
                stages = []

                [providers.main]
                api = "responses"
                base_url = "https://api.example.com/v1"

                [assistants.test]
                provider = "main"
                model = "gpt-test"
                window = 1
                "#,
                    )
                    .unwrap(),
                )
                .await
                .unwrap()
            })
            .await
            .assistant("test")
            .unwrap()
    }

    #[tokio::test]
    async fn visible_parent_error_creates_notification() {
        let mut app = App::new(crate::project::Project::new_test().unwrap());
        let aid = AgentId::from("a".to_string());
        app.tabs.insert(aid.clone(), TabEntry::Loading);

        app.handle_parent_event(
            aid,
            ParentEvent::Error("oops".into()),
        )
        .await
        .unwrap();

        let notification = app.notification.expect("expected notification");
        assert!(matches!(notification.kind, NotificationKind::Error));
        assert_eq!(notification.msg, "oops");
    }

    #[tokio::test]
    async fn hidden_parent_error_is_ignored() {
        let mut app = App::new(crate::project::Project::new_test().unwrap());

        app.handle_parent_event(
            AgentId::from("hidden".to_string()),
            ParentEvent::Error("oops".into()),
        )
        .await
        .unwrap();

        assert!(app.notification.is_none());
    }

    #[tokio::test]
    async fn assistant_set_updates_tab_state() {
        let project = crate::project::Project::new_test().unwrap();
        let mut app = App::new(project.clone());
        let aid = AgentId::from(format!("assistant-set-{}", uuid::Uuid::new_v4()));
        let workdir = project.agent_workdir(&aid);
        std::fs::create_dir_all(&workdir).unwrap();
        Repository::init(&workdir).unwrap();
        let assistant = test_assistant().await;
        let state = AgentState {
            status: AgentStatus::default(),
            assistant: assistant.clone(),
            visibility: AgentVisibility::Tab,
            context: crate::agent::AgentContext {
                commit: "".into(),
                history: History::new("".into()),
            },
        };
        let tab = Tab::new(
            crate::agent::router::AgentRouter::test_handle(),
            aid.clone(),
            state,
            &project,
        )
        .unwrap();
        app.tabs.insert(aid.clone(), TabEntry::Ready(tab));

        app.handle_parent_event(aid.clone(), ParentEvent::AssistantSet(assistant.clone()))
            .await
            .unwrap();

        let tab = app.tab_mut_by_aid(&aid).unwrap();
        assert_eq!(tab.state.assistant.id, assistant.id);

        std::fs::remove_dir_all(project.agent(&aid)).ok();
    }
}

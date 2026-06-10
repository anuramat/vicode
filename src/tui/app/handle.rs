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
            NewAgent(agent_id, state) => {
                if let Err(e) = self.new_agent(agent_id.clone(), *state).await {
                    // drop the preview tab instead of leaving a router-less zombie
                    self.tabs.shift_remove(&agent_id);
                    self.rebuild_tablist();
                    return Err(e);
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
        aid: AgentId,
        event: ParentEvent,
    ) -> Result<()> {
        #[allow(clippy::enum_glob_use)]
        use ParentEvent::*;

        let tab = self.tab_mut_by_aid(&aid)?;
        match event {
            Started(state) => {
                // TODO this calls tab_mut_by_aid again, which is sad
                self.handle_started(&aid, *state).await?;
            }
            HistoryUpdate(loc, event) => {
                tab.update(loc, event)?;
            }
            Error(msg) => {
                self.notify(NotificationKind::Error, msg);
            }
            StatusUpdate(status) => {
                if tab.set_state(status).await? {
                    tab.refresh_info().await?;
                }
            }
            AssistantSet(assistant) => {
                tab.state.assistant = assistant;
                self.tx.send(AppEvent::TabStatusChanged(aid)).await?;
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
    use crate::config::Config;
    use crate::llm::history::AssistantEvent;
    use crate::llm::history::History;
    use crate::llm::history::HistoryUpdate;
    use crate::llm::history::delta::Delta;
    use crate::llm::history::delta::DeltaContent;
    use crate::llm::history::message::UserMessage;
    use crate::llm::provider::assistant::Assistant;
    use crate::llm::provider::assistant::AssistantPool;
    use crate::project::layout::LayoutTrait;
    use crate::tui::app::NotificationKind;
    use crate::tui::tab::Tab;

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
        let mut app = App::new(
            crate::project::Project::new_test().unwrap(),
            Default::default(),
        );
        let aid = AgentId::from("a".to_string());
        let state = AgentState {
            status: AgentStatus::default(),
            assistant: test_assistant().await,
            max_depth: 1,
            context: crate::agent::AgentContext {
                commit: "".into(),
                history: History::new("".into()),
            },
        };
        app.tabs.insert(
            aid.clone(),
            Tab::new(None, aid.clone(), state, &app.project),
        );

        app.handle_parent_event(aid, ParentEvent::Error("oops".into()))
            .await
            .unwrap();

        let notification = app.notification.expect("expected notification");
        assert!(matches!(notification.kind, NotificationKind::Error));
        assert_eq!(notification.msg, "oops");
    }

    #[tokio::test]
    async fn assistant_set_updates_tab_state() {
        let project = crate::project::Project::new_test().unwrap();
        let mut app = App::new(project.clone(), Default::default());
        let aid = AgentId::from(format!("assistant-set-{}", uuid::Uuid::new_v4()));
        let workdir = project.agent_workdir(&aid);
        std::fs::create_dir_all(&workdir).unwrap();
        Repository::init(&workdir).unwrap();
        let assistant = test_assistant().await;
        let state = AgentState {
            status: AgentStatus::default(),
            assistant: assistant.clone(),
            max_depth: 1,
            context: crate::agent::AgentContext {
                commit: "".into(),
                history: History::new("".into()),
            },
        };
        let tab = Tab::new(
            Some(crate::agent::router::AgentRouter::test_handle()),
            aid.clone(),
            state,
            &project,
        );
        app.tabs.insert(aid.clone(), tab);

        app.handle_parent_event(aid.clone(), ParentEvent::AssistantSet(assistant.clone()))
            .await
            .unwrap();

        let tab = app.tab_mut_by_aid(&aid).unwrap();
        assert_eq!(tab.state.assistant.id, assistant.id);

        std::fs::remove_dir_all(project.agent(&aid)).ok();
    }

    #[tokio::test]
    async fn tab_history_replays_authoritative_history_updates_exactly() {
        let project = crate::project::Project::new_test().unwrap();
        let mut app = App::new(project.clone(), Default::default());
        let aid = AgentId::from("deterministic-tab".to_string());
        let state = AgentState {
            status: AgentStatus::default(),
            assistant: test_assistant().await,
            max_depth: 1,
            context: crate::agent::AgentContext {
                commit: "".into(),
                history: History::new("instructions".into()),
            },
        };
        app.tabs.insert(
            aid.clone(),
            Tab::new(
                Some(crate::agent::router::AgentRouter::test_handle()),
                aid.clone(),
                state.clone(),
                &project,
            ),
        );
        let events = vec![
            HistoryUpdate::UserMessage(UserMessage::new("hi".into(), 1)),
            HistoryUpdate::GenerationIncremented,
            HistoryUpdate::TurnResponse(AssistantEvent::Created { created_at: 2 }),
            HistoryUpdate::TurnResponse(AssistantEvent::Started { started_at: 3 }),
            HistoryUpdate::TurnResponse(AssistantEvent::Delta(Delta {
                id: "out".into(),
                delta: DeltaContent::Output("hello".into()),
                timestamp: 4,
            })),
            HistoryUpdate::TurnResponse(AssistantEvent::Completed { ended_at: 5 }),
        ];
        let mut expected = state.context.history.clone();

        for event in events {
            let generation = expected.generation();
            expected.handle(generation, event.clone()).unwrap();
            app.handle_parent_event(aid.clone(), ParentEvent::HistoryUpdate(generation, event))
                .await
                .unwrap();
        }

        let actual = &app.tab_mut_by_aid(&aid).unwrap().state.context.history;
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );

        std::fs::remove_dir_all(project.agent(&aid)).ok();
    }
}

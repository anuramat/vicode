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
            ParentEvent(agent_id, event) => {
                self.handle_parent_event(agent_id, event).await?;
                self.dirty = true;
            }
            // the failed copy's preview tab must not linger (M5)
            DuplicateFailed(copy) => {
                self.tabs.shift_remove(&copy);
                self.rebuild_tablist();
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

        // hidden children have no tab; drop their events silently
        let Ok(tab) = self.tab_mut_by_aid(&aid) else {
            return Ok(());
        };
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
                if tab.set_state(status)? {
                    tab.refresh_info().await?;
                    self.rebuild_tablist();
                }
            }
            AssistantSet(assistant) => {
                tab.state.assistant = assistant;
                tab.refresh_assistant_config();
                self.rebuild_tablist();
            }
            ToolOutput { call_id, chunk } => {
                tab.stream_tool_output(call_id, &chunk);
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
    use crate::llm::history::AssistantEvent;
    use crate::llm::history::HistoryUpdate;
    use crate::llm::history::delta::Delta;
    use crate::llm::history::delta::DeltaContent;
    use crate::llm::history::message::UserMessage;
    use crate::tui::app::NotificationKind;
    use crate::tui::tab::Tab;

    #[tokio::test]
    async fn visible_parent_error_creates_notification() {
        let mut app = App::new(
            crate::project::Project::new_test().unwrap().0,
            Default::default(),
            Default::default(),
        );
        let aid = AgentId::from("a".to_string());
        let state = AgentState::fake();
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
        let project = crate::project::Project::new_test().unwrap().0;
        let mut app = App::new(project.clone(), Default::default(), Default::default());
        let aid = AgentId::from(format!("assistant-set-{}", uuid::Uuid::new_v4()));
        let workdir = project.agent_workdir(&aid);
        std::fs::create_dir_all(&workdir).unwrap();
        Repository::init(&workdir).unwrap();
        let state = AgentState::fake();
        let tab = Tab::new(
            Some(crate::agent::router::AgentRouter::test_handle()),
            aid.clone(),
            state,
            &project,
        );
        app.tabs.insert(aid.clone(), tab);

        app.handle_parent_event(aid.clone(), ParentEvent::AssistantSet("test".into()))
            .await
            .unwrap();

        let tab = app.tab_mut_by_aid(&aid).unwrap();
        assert_eq!(tab.state.assistant, "test");

        std::fs::remove_dir_all(project.agent(&aid)).ok();
    }

    #[tokio::test]
    async fn duplicate_failed_rolls_back_preview_tab() {
        let project = crate::project::Project::new_test().unwrap().0;
        let mut app = App::new(project.clone(), Default::default(), Default::default());
        let original = AgentId::from("orig".to_string());
        let copy = AgentId::from("copy".to_string());
        let state = AgentState::fake();
        for aid in [&original, &copy] {
            app.tabs.insert(
                aid.clone(),
                Tab::new(None, aid.clone(), state.clone(), &project),
            );
        }
        app.rebuild_tablist();

        app.handle(AppEvent::DuplicateFailed(copy.clone()))
            .await
            .unwrap();

        assert!(!app.tabs.contains_key(&copy));
        assert_eq!(app.tabs.len(), 1);
    }

    /// H1: the app loop is the sole receiver of its own channel — handlers
    /// must complete even when that channel is saturated by agent emits
    #[tokio::test]
    async fn parent_event_handlers_never_block_on_a_full_app_channel() {
        let project = crate::project::Project::new_test().unwrap().0;
        let mut app = App::new(project.clone(), Default::default(), Default::default());
        let aid = AgentId::from(format!("full-chan-{}", uuid::Uuid::new_v4()));
        let workdir = project.agent_workdir(&aid);
        std::fs::create_dir_all(&workdir).unwrap();
        Repository::init(&workdir).unwrap();
        let tab = Tab::new(
            Some(crate::agent::router::AgentRouter::test_handle()),
            aid.clone(),
            AgentState::fake(),
            &project,
        );
        app.tabs.insert(aid.clone(), tab);
        app.rebuild_tablist();
        while app.tx.try_send(AppEvent::Redraw).is_ok() {}

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            app.handle_parent_event(
                aid.clone(),
                ParentEvent::StatusUpdate(crate::agent::ActivityStatus::Normal(
                    crate::llm::history::TurnStatus::InProgress,
                )),
            )
            .await
            .unwrap();
            app.handle_parent_event(aid.clone(), ParentEvent::AssistantSet("test".into()))
                .await
                .unwrap();
        })
        .await
        .expect("handler blocked on the full app channel");

        std::fs::remove_dir_all(project.agent(&aid)).ok();
    }

    #[tokio::test]
    async fn tool_output_streams_into_tab_live_buffer() {
        let project = crate::project::Project::new_test().unwrap().0;
        let mut app = App::new(project.clone(), Default::default(), Default::default());
        let aid = AgentId::from("streamer".to_string());
        app.tabs.insert(
            aid.clone(),
            Tab::new(None, aid.clone(), AgentState::fake(), &project),
        );

        for chunk in ["one", " two"] {
            app.handle_parent_event(
                aid.clone(),
                ParentEvent::ToolOutput {
                    call_id: "c1".into(),
                    chunk: chunk.into(),
                },
            )
            .await
            .unwrap();
        }

        let tab = app.tab_mut_by_aid(&aid).unwrap();
        assert_eq!(tab.live_output["c1"], "one two");
    }

    #[tokio::test]
    async fn tab_history_replays_authoritative_history_updates_exactly() {
        let project = crate::project::Project::new_test().unwrap().0;
        let mut app = App::new(project.clone(), Default::default(), Default::default());
        let aid = AgentId::from("deterministic-tab".to_string());
        let state = AgentState::fake();
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

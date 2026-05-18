use anyhow::Result;

use crate::agent::Agent;
use crate::agent::handle::ParentEvent;
use crate::agent::task::manager::TaskId;

impl Agent {
    pub async fn handle_task_result(
        &mut self,
        id: TaskId,
        event: Result<()>,
    ) -> Result<()> {
        if let Err(ref err) = event {
            self.emit(ParentEvent::Error(err.to_string())).await?;
        }
        let applied = self.tskmgr.finish_task(&id);
        if !applied {
            return Ok(());
        }
        if self.tskmgr.idle() {
            if self.history().state().needs_another_turn() && !self.history().compacting() {
                self.start_turn().await?;
            } else {
                self.fire_pending_done();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::Receiver;
    use tokio::sync::mpsc::channel;
    use tokio::time::Duration;
    use tokio::time::timeout;

    use super::*;
    use crate::agent::AgentId;
    use crate::agent::AgentState;
    use crate::agent::handle::AgentEvent;
    use crate::agent::handle::ParentEvent;
    use crate::config::Config;
    use crate::llm::history::AssistantEvent;
    use crate::llm::history::History;
    use crate::llm::history::HistoryUpdate;
    use crate::llm::provider::assistant::Assistant;
    use crate::llm::provider::assistant::AssistantPool;
    use crate::project::Project;
    use crate::project::layout::LayoutTrait;
    use crate::tui::app::AppEvent;

    const RX_TIMEOUT: Duration = Duration::from_secs(1);

    async fn recv<T>(
        rx: &mut Receiver<T>,
        name: &str,
    ) -> T {
        timeout(RX_TIMEOUT, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {name}"))
            .unwrap_or_else(|| panic!("{name} channel closed"))
    }

    fn parent_event(event: AppEvent) -> ParentEvent {
        match event {
            AppEvent::ParentEvent(_, event) => event,
            other => panic!("expected ParentEvent, got {other:?}"),
        }
    }

    async fn assistant() -> Assistant {
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
                window = 99999
                "#,
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .assistant("test")
        .unwrap()
    }

    #[tokio::test]
    async fn compact_failure_does_not_start_normal_turn() {
        let project = Project::new_test().unwrap();
        let aid = AgentId::from(format!("compact-failed-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(project.agent(&aid))
            .await
            .unwrap();
        let (parent_tx, mut parent_rx) = channel(8);
        let (tx, rx) = channel(8);
        let assistant = assistant().await;
        let mut agent = Agent {
            project: project.clone(),
            id: aid.clone(),
            state: AgentState {
                status: Default::default(),
                assistant: assistant.clone(),
                max_depth: 1,
                context: crate::agent::AgentContext {
                    commit: "".into(),
                    history: History::new("".into()),
                },
            },
            router: crate::agent::router::AgentRouter::test_handle_with_app_tx(parent_tx),
            pending_done: None,
            tx,
            rx,
            tskmgr: crate::agent::task::manager::AgentTaskManager::new(),
            tools: Default::default(),
        };
        agent
            .state
            .context
            .history
            .handle(0, HistoryUpdate::UserMessage("first".into()))
            .unwrap();
        agent
            .state
            .context
            .history
            .handle(0, HistoryUpdate::CompactStart { n_drop: 1 })
            .unwrap();
        agent
            .state
            .context
            .history
            .handle(
                0,
                HistoryUpdate::CompactResponse(AssistantEvent::Created(0)),
            )
            .unwrap();
        agent
            .state
            .context
            .history
            .handle(
                0,
                HistoryUpdate::CompactResponse(AssistantEvent::Failed("oops".into())),
            )
            .unwrap();
        agent
            .tskmgr
            .spawn(agent.tx.clone(), 0, |_| async { Ok(()) });
        let event = recv(&mut agent.rx, "task completion").await;
        assert!(matches!(event, AgentEvent::TaskDone(..)));

        let _ = agent.handle(event).await.unwrap();

        assert!(agent.tskmgr.idle());
        assert!(agent.state.context.history.compacting());
        let event = parent_event(recv(&mut parent_rx, "parent event").await);
        assert!(
            matches!(
                event,
                ParentEvent::StatusUpdate(crate::agent::AgentStatus::Compact(
                    crate::llm::history::TurnStatus::Failed(ref msg),
                ))
                    if msg == "oops"
            ),
            "{event:?}"
        );

        tokio::fs::remove_dir_all(project.agent(&aid))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn pre_stream_failure_keeps_error_status_until_turn_complete() {
        let project = Project::new_test().unwrap();
        let aid = AgentId::from(format!("pre-stream-failed-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(project.agent(&aid))
            .await
            .unwrap();
        let (parent_tx, mut parent_rx) = channel(8);
        let (tx, rx) = channel(8);
        let assistant = assistant().await;
        let mut agent = Agent {
            project: project.clone(),
            id: aid.clone(),
            state: AgentState {
                status: crate::agent::AgentStatus::Normal(
                    crate::llm::history::TurnStatus::InProgress,
                ),
                assistant: assistant.clone(),
                max_depth: 1,
                context: crate::agent::AgentContext {
                    commit: "".into(),
                    history: History::new("".into()),
                },
            },
            router: crate::agent::router::AgentRouter::test_handle_with_app_tx(parent_tx),
            pending_done: None,
            tx,
            rx,
            tskmgr: crate::agent::task::manager::AgentTaskManager::new(),
            tools: Default::default(),
        };
        agent
            .state
            .context
            .history
            .handle(0, HistoryUpdate::UserMessage("first".into()))
            .unwrap();
        agent
            .handle_history(0, HistoryUpdate::TurnResponse(AssistantEvent::Created(0)))
            .await
            .unwrap();
        agent
            .handle_history(
                0,
                HistoryUpdate::TurnResponse(AssistantEvent::Failed("oops".into())),
            )
            .await
            .unwrap();
        agent.tskmgr.spawn(agent.tx.clone(), 0, |_| async {
            Err(anyhow::anyhow!("oops"))
        });
        let event = recv(&mut agent.rx, "task completion").await;
        assert!(matches!(event, AgentEvent::TaskDone(..)));

        let _ = agent.handle(event).await.unwrap();

        assert!(matches!(
            agent.state.status,
            crate::agent::AgentStatus::Normal(crate::llm::history::TurnStatus::Failed(ref msg))
                if msg == "oops"
        ));
        assert!(matches!(
            agent.state.context.history.state().last(),
            Some(crate::llm::history::message::Message::Assistant(crate::llm::history::message::AssistantMessage {
                status: crate::llm::history::message::AssistantStatus::Error(msg),
                ..
            })) if msg == "oops"
        ));
        let events = [
            parent_event(recv(&mut parent_rx, "parent event").await),
            parent_event(recv(&mut parent_rx, "parent event").await),
            parent_event(recv(&mut parent_rx, "parent event").await),
            parent_event(recv(&mut parent_rx, "parent event").await),
        ];
        assert!(
            matches!(
                events.as_slice(),
                [
                    ParentEvent::HistoryUpdate(
                        _,
                        HistoryUpdate::TurnResponse(AssistantEvent::Created(_))
                    ),
                    ParentEvent::HistoryUpdate(
                        _,
                        HistoryUpdate::TurnResponse(AssistantEvent::Failed(msg))
                    ),
                    ParentEvent::Error(error),
                    ParentEvent::StatusUpdate(crate::agent::AgentStatus::Normal(
                        crate::llm::history::TurnStatus::Failed(status),
                    )),
                ] if msg == "oops" && error == "oops" && status == "oops"
            ),
            "{events:?}"
        );

        tokio::fs::remove_dir_all(project.agent(&aid))
            .await
            .unwrap();
    }
}

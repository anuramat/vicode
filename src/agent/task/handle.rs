use anyhow::Result;

use crate::agent::Agent;
use crate::agent::AgentStatus;
use crate::agent::handle::ParentEvent;
use crate::agent::task::manager::TaskId;

impl Agent {
    pub async fn handle_task_result(
        &mut self,
        id: TaskId,
        event: Result<()>,
    ) -> Result<()> {
        if let Err(ref err) = event {
            self.parent
                .send(ParentEvent::Error(err.to_string()))
                .await?;
            self.set_status(AgentStatus::Error(err.to_string())).await?;
        }
        let applied = self.tskmgr.finish_task(&id);
        if !applied {
            return Ok(());
        }
        if self.tskmgr.idle() {
            if self.state.context.history.needs_another_turn()
                && self.state.context.history.compact.is_none()
            {
                self.start_turn();
            } else {
                self.sync_status().await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::agent::AgentContext;
    use crate::agent::AgentId;
    use crate::agent::AgentState;
    use crate::agent::AgentTopology;
    use crate::agent::handle::AgentEvent;
    use crate::agent::handle::ParentEvent;
    use crate::agent::init::channel_parent_sink;
    use crate::config::Config;
    use crate::llm::history::HistoryUpdate;
    use crate::llm::history::ResponseEvent;
    use crate::llm::provider::assistant::Assistant;
    use crate::llm::provider::assistant::AssistantPool;
    use crate::project::PROJECT;
    use crate::project::layout::LayoutTrait;

    async fn assistant() -> Assistant {
        AssistantPool::from_config(
            &Config::parse(
                r#"
                primary_assistant = ["test"]
                shell_cmd = ["bash", "-c"]

                [sandbox]
                kind = "bwrap"
                bin = "bwrap"
                args = []
                stages = []

                [keymap.cmdline]

                [keymap.normal]

                [keymap.insert]

                [providers.main]
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
        let aid = AgentId::from(format!("compact-failed-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(PROJECT.agent(&aid))
            .await
            .unwrap();
        let (parent_tx, mut parent_rx) = channel(8);
        let (tx, rx) = channel(8);
        let assistant = assistant().await;
        let mut agent = Agent {
            id: aid.clone(),
            state: AgentState {
                status: Default::default(),
                assistant: assistant.clone(),
                topology: AgentTopology::default(),
                context: AgentContext {
                    ..Default::default()
                },
            },
            parent: channel_parent_sink(parent_tx),
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
            .handle(
                0,
                HistoryUpdate::CompactStart {
                    dropped: 1,
                    needs_another_turn: true,
                },
            )
            .unwrap();
        agent
            .state
            .context
            .history
            .handle(0, HistoryUpdate::CompactResponse(ResponseEvent::Started(0)))
            .unwrap();
        agent
            .state
            .context
            .history
            .handle(
                0,
                HistoryUpdate::CompactResponse(ResponseEvent::Failed("oops".into())),
            )
            .unwrap();
        agent
            .tskmgr
            .spawn(agent.tx.clone(), 0, |_| async { Ok(()) });
        let Some(AgentEvent::TaskDone(tid, result)) = agent.rx.recv().await else {
            panic!("expected task completion");
        };

        agent.handle_task_result(tid, result).await.unwrap();

        assert!(agent.tskmgr.idle());
        assert!(agent.state.context.history.compact.is_some());
        let event = parent_rx.recv().await;
        assert!(
            matches!(
                event,
                Some(ParentEvent::StatusUpdate(crate::agent::AgentStatus::Error(ref msg)))
                    if msg == "oops"
            ),
            "{event:?}"
        );

        tokio::fs::remove_dir_all(PROJECT.agent(&aid))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn pre_stream_failure_keeps_error_status_until_turn_complete() {
        let aid = AgentId::from(format!("pre-stream-failed-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(PROJECT.agent(&aid))
            .await
            .unwrap();
        let (parent_tx, mut parent_rx) = channel(8);
        let (tx, rx) = channel(8);
        let assistant = assistant().await;
        let mut agent = Agent {
            id: aid.clone(),
            state: AgentState {
                status: crate::agent::AgentStatus::InProgress,
                assistant: assistant.clone(),
                topology: AgentTopology::default(),
                context: AgentContext {
                    ..Default::default()
                },
            },
            parent: channel_parent_sink(parent_tx),
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
            .handle_history(
                0,
                HistoryUpdate::TurnResponse(ResponseEvent::Failed("oops".into())),
            )
            .await
            .unwrap();
        agent.tskmgr.spawn(agent.tx.clone(), 0, |_| async {
            Err(anyhow::anyhow!("oops"))
        });
        let Some(AgentEvent::TaskDone(tid, result)) = agent.rx.recv().await else {
            panic!("expected task completion");
        };

        agent.handle_task_result(tid, result).await.unwrap();

        assert!(
            matches!(agent.state.status, crate::agent::AgentStatus::Error(ref msg) if msg == "oops")
        );
        assert!(matches!(
            agent.state.context.history.last().map(|entry| &entry.message),
            Some(crate::llm::message::Message::User(crate::llm::message::UserMessage { text })) if text == "first"
        ));
        let events = [
            parent_rx.recv().await,
            parent_rx.recv().await,
            parent_rx.recv().await,
        ];
        assert!(
            matches!(
                events.as_slice(),
                [
                    Some(ParentEvent::HistoryUpdate(
                        _,
                        HistoryUpdate::TurnResponse(ResponseEvent::Failed(msg))
                    )),
                    Some(ParentEvent::Error(error)),
                    Some(ParentEvent::StatusUpdate(crate::agent::AgentStatus::Error(status))),
                ] if msg == "oops" && error == "oops" && status == "oops"
            ),
            "{events:?}"
        );

        tokio::fs::remove_dir_all(PROJECT.agent(&aid))
            .await
            .unwrap();
    }
}

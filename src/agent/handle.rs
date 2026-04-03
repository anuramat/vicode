use std::ops::ControlFlow;

use anyhow::Result;
use futures::future::try_join_all;
use tracing::debug;
use tracing::error;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentHandle;
use crate::agent::AgentStatus;
use crate::agent::id::AgentId;
use crate::agent::replica;
use crate::agent::task::manager::TaskId;
use crate::agent::task::sink::TurnHandle;
use crate::agent::task::sink::TurnType;
use crate::llm::history;
use crate::llm::history::History;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::ResponseEvent;
use crate::llm::message::AssistantItem;
use crate::llm::message::DeveloperMessage;
use crate::llm::message::SubagentReportMessage;
use crate::llm::message::now_ms;
use crate::llm::provider::assistant::ASSISTANT_POOL;

const ABORTED_BY_USER: &str = "aborted by user";

#[derive(Debug)]
pub enum AgentEvent {
    TaskDone(TaskId, Result<()>),
    TaskEvent(TaskId, HistoryGeneration, HistoryUpdate),
    External(ExternalEvent),
}

#[derive(Debug)]
pub enum ParentEvent {
    Started(Box<AgentHandle>),
    HistoryReset(History),
    HistoryUpdate(HistoryGeneration, HistoryUpdate),
    // XXX maybe status update should be a variant of history update?
    StatusUpdate(AgentStatus),
    Error(String),
}

#[derive(Debug)]
pub enum ExternalEvent {
    /// Compact the first n messages
    Compact(usize),
    Delete,
    Retry,
    Abort,
    Undo(usize), // TODO maybe this should send generation or whatever
    SetAssistant(String),
    Submit(UserPrompt),
    DuplicateRequest(AgentId),
}

#[async_trait::async_trait]
pub trait ParentSink: Send + Sync {
    async fn send(
        &self,
        event: ParentEvent,
    ) -> Result<()>;

    fn sibling(
        &self,
        aid: AgentId,
    ) -> ParentHandle;
}

pub type ParentHandle = Box<dyn ParentSink>;

#[derive(Debug)]
pub struct UserPrompt {
    pub text: Option<String>,
    pub multiplier: usize,
    pub generation: HistoryGeneration,
}

impl Agent {
    pub async fn set_status(
        &mut self,
        status: AgentStatus,
    ) -> Result<()> {
        // TODO move ParentEvent::Error event emission here?
        if self.state.status == status {
            return Ok(());
        }
        self.state.status = status.clone();
        self.parent.send(ParentEvent::StatusUpdate(status)).await?;
        Ok(())
    }

    // XXX try to drop this
    pub async fn sync_status(&mut self) -> Result<()> {
        let status = match AgentStatus::from_history(&self.state.context.history) {
            AgentStatus::Idle if matches!(self.state.status, AgentStatus::Error(_)) => {
                self.state.status.clone()
            }
            status => status,
        };
        self.set_status(status).await
    }

    #[instrument(skip(self))]
    pub async fn handle(
        &mut self,
        event: AgentEvent,
    ) -> Result<ControlFlow<()>> {
        use AgentEvent::*;

        debug!(event = ?event, "handling agent event");

        match event {
            TaskDone(tid, result) => {
                self.handle_task_result(tid, result).await?;
            }
            TaskEvent(tid, generation, event) => {
                if self.tskmgr.pending(&tid) {
                    self.handle_history(generation, event).await?;
                }
            }
            External(event) => {
                return self.handle_external(event).await;
            }
        }
        Ok(ControlFlow::Continue(()))
    }

    pub fn idle(&mut self) -> Result<()> {
        anyhow::ensure!(self.tskmgr.idle(), "agent is busy");
        Ok(())
    }

    async fn increment_generation(&mut self) -> Result<HistoryGeneration> {
        let generation = self.state.context.history.generation();
        self.handle_history(generation, HistoryUpdate::GenerationIncremented)
            .await?;
        Ok(self.state.context.history.generation())
    }

    async fn handle_external(
        &mut self,
        event: ExternalEvent,
    ) -> Result<ControlFlow<()>> {
        use ExternalEvent::*;
        match event {
            Undo(n) => {
                self.idle()?;
                let g = self.increment_generation().await?;
                self.handle_history(g, HistoryUpdate::Pop(n)).await?;
            }
            Abort => {
                self.tskmgr.abort().await;
                let g = self.increment_generation().await?;
                let event = if self.state.context.history.compacting() {
                    HistoryUpdate::CompactResponse(ResponseEvent::Failed(ABORTED_BY_USER.into()))
                } else {
                    HistoryUpdate::TurnResponse(ResponseEvent::Failed(ABORTED_BY_USER.into()))
                };
                self.handle_history(g, event).await?;
                self.sync_status().await?;
            }
            Delete => {
                self.tskmgr.abort().await;
                self.delete_agent().await?; // TODO maybe some special handling for failed deletes
                return Ok(ControlFlow::Break(()));
            }
            DuplicateRequest(aid) => {
                self.idle()?;
                self.try_duplicate(aid).await?;
            }
            Submit(UserPrompt {
                text,
                multiplier,
                generation,
            }) => {
                // XXX verify generation logic
                self.idle()?;
                if let Some(text) = text {
                    self.handle_history(generation, history::HistoryUpdate::UserMessage(text))
                        .await?;
                }
                self.increment_generation().await?;
                self.set_status(AgentStatus::InProgress).await?;
                if multiplier <= 1 {
                    self.start_turn();
                } else {
                    let replicas = try_join_all((0..multiplier).map(|_| AgentId::new())).await?;
                    self.state.topology.children.extend(replicas.clone());
                    self.save().await?;
                    self.start_replica_turns(replicas);
                }
            }
            Compact(n) => {
                self.idle()?;
                self.init_compact(n).await?;
                self.compact_turn().await?;
            }
            Retry => {
                self.idle()?;
                self.increment_generation().await?;
                if self.state.context.history.compact.is_some() {
                    self.compact_turn().await?;
                } else {
                    self.set_status(AgentStatus::InProgress).await?;
                    self.start_turn();
                }
            }
            SetAssistant(id) => {
                self.set_assistant(&id).await?;
            }
        }
        Ok(ControlFlow::Continue(()))
    }

    pub async fn handle_history(
        &mut self,
        generation: HistoryGeneration,
        event: HistoryUpdate,
    ) -> Result<()> {
        // TODO verify
        self.parent
            .send(ParentEvent::HistoryUpdate(generation, event.clone()))
            .await?;
        self.state
            .context
            .history
            .handle(generation, event.clone())?;
        match event {
            HistoryUpdate::GenerationIncremented => return Ok(()),
            HistoryUpdate::TurnResponse(ResponseEvent::Item(ref item)) => {
                self.execute_tool_calls(item).await?
            }
            HistoryUpdate::TurnResponse(ResponseEvent::Failed(msg))
            | HistoryUpdate::CompactResponse(ResponseEvent::Failed(msg)) => {
                error!("response error in agent {}: {}", self.id, msg);
            }
            HistoryUpdate::TurnResponse(ResponseEvent::Delta(_))
            | HistoryUpdate::CompactResponse(ResponseEvent::Delta(_)) => return Ok(()),
            _ => {}
        }
        // TODO save less often; save on errors
        self.save().await?;
        Ok(())
    }

    fn start_replica_turns(
        &mut self,
        replicas: Vec<AgentId>,
    ) {
        let parent = self.id.clone();
        let context = self.state.context.clone();
        let assistant = self.state.assistant.clone();
        self.tskmgr.spawn(
            self.tx.clone(),
            context.history.generation(),
            move |task| async move {
                let result = replica::run_replicas(parent, context, assistant, replicas).await?;
                task.send(HistoryUpdate::DeveloperMessage(
                    DeveloperMessage::SubagentReport(SubagentReportMessage {
                        text: result.report,
                    }),
                ))
                .await
            },
        );
    }

    pub async fn set_assistant(
        &mut self,
        id: &str,
    ) -> Result<()> {
        self.state.assistant = ASSISTANT_POOL.get().unwrap().assistant(id)?;
        self.save().await
    }

    pub async fn execute_tool_calls(
        &mut self,
        item: &AssistantItem,
    ) -> Result<()> {
        if let AssistantItem::ToolCall(mut call) = item.clone()
            && call.task.output().is_none()
        {
            let generation = self.state.context.history.generation();
            call.task.prepare(self)?;
            self.tskmgr
                .spawn(self.tx.clone(), generation, move |task| async move {
                    let handle = TurnHandle {
                        task,
                        turn_type: TurnType::Default,
                    };
                    call.task.run().await;
                    call.executed_at_ms = Some(now_ms());
                    handle
                        .send(ResponseEvent::Item(Box::new(AssistantItem::ToolCall(call))))
                        .await
                });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use futures::future::pending;
    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::agent::AgentState;
    use crate::agent::init::channel_parent_sink;
    use crate::config::Config;
    use crate::llm::provider::assistant::Assistant;
    use crate::llm::provider::assistant::AssistantPool;
    use crate::project::PROJECT;
    use crate::project::layout::LayoutTrait;

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
        .assistant("test")
        .unwrap()
    }

    #[tokio::test]
    async fn abort_emits_turn_complete_and_marks_history_failed() {
        let aid = AgentId::from(format!("abort-test-{}", uuid::Uuid::new_v4()));
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
                topology: Default::default(),
                context: crate::agent::AgentContext {
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
            .handle(0, HistoryUpdate::TurnResponse(ResponseEvent::Started(0)))
            .unwrap();
        agent.tskmgr.spawn(agent.tx.clone(), 0, |_| async move {
            pending::<Result<()>>().await
        });

        let _ = agent.handle_external(ExternalEvent::Abort).await.unwrap();

        let events = [
            parent_rx.recv().await.unwrap(),
            parent_rx.recv().await.unwrap(),
            parent_rx.recv().await.unwrap(),
        ];
        assert!(matches!(
            events.as_slice(),
            [
                ParentEvent::HistoryUpdate(_, HistoryUpdate::GenerationIncremented),
                ParentEvent::HistoryUpdate(_, HistoryUpdate::TurnResponse(ResponseEvent::Failed(msg))),
                ParentEvent::StatusUpdate(crate::agent::AgentStatus::Error(status)),
            ] if msg == ABORTED_BY_USER && status == ABORTED_BY_USER
        ));
        assert!(matches!(
            agent.state.context.history.last().map(|entry| &entry.message),
            Some(crate::llm::message::Message::Assistant(crate::llm::message::AssistantMessage {
                finish_reason: crate::llm::message::AssistantMessageStatus::Error(msg),
                ..
            })) if msg == ABORTED_BY_USER
        ));

        tokio::fs::remove_dir_all(PROJECT.agent(&aid))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn retry_after_compact_failure_restarts_compaction() {
        let aid = AgentId::from(format!("compact-retry-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(PROJECT.agent(&aid))
            .await
            .unwrap();
        let (parent_tx, _parent_rx) = channel(8);
        let (tx, rx) = channel(8);
        let assistant = assistant().await;
        let mut agent = Agent {
            id: aid.clone(),
            state: AgentState {
                status: Default::default(),
                assistant: assistant.clone(),
                topology: Default::default(),
                context: crate::agent::AgentContext {
                    ..Default::default()
                },
            },
            parent: channel_parent_sink(parent_tx),
            tx,
            rx,
            tskmgr: crate::agent::task::manager::AgentTaskManager::new(),
            tools: Default::default(),
        };
        let history = &mut agent.state.context.history;
        history
            .handle(
                0,
                HistoryUpdate::DeveloperMessage(DeveloperMessage::new("x".repeat(2000))),
            )
            .unwrap();
        history
            .handle(
                0,
                HistoryUpdate::CompactStart {
                    dropped: 1,
                    needs_another_turn: false,
                },
            )
            .unwrap();
        agent
            .handle_history(
                0,
                HistoryUpdate::CompactResponse(ResponseEvent::Failed("oops".into())),
            )
            .await
            .unwrap();

        let _ = agent.handle_external(ExternalEvent::Retry).await.unwrap();

        assert!(agent.state.context.history.compact.is_some());

        tokio::fs::remove_dir_all(PROJECT.agent(&aid))
            .await
            .unwrap();
    }
}

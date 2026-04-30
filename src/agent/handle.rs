use std::fmt::Debug;
use std::ops::ControlFlow;

use anyhow::Result;
use futures::StreamExt;
use futures::TryStreamExt;
use futures::stream;
use tracing::debug;
use tracing::error;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentHandle;
use crate::agent::AgentStatus;
use crate::agent::id::AgentId;
use crate::agent::subagent;
use crate::agent::subagent::SubagentResult;
use crate::agent::subagent::replica;
use crate::agent::task::manager::TaskId;
use crate::agent::task::sink::TurnHandle;
use crate::agent::task::sink::TurnType;
use crate::llm::history;
use crate::llm::history::AssistantEvent;
use crate::llm::history::History;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::utils::now;

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
    SubagentDone(SubagentResult),
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
pub trait ParentSink: Send + Sync + Debug {
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
    pub text: String,
    pub multiplier: usize,
    pub generation: HistoryGeneration,
}

impl Agent {
    pub fn derive_status(&self) -> AgentStatus {
        let busy = !self.tskmgr.idle();
        match self.history().activity() {
            history::Activity::Normal { state } => AgentStatus::Normal(state.turn_status(busy)),
            history::Activity::Compacting { compact, .. } => {
                AgentStatus::Compact(compact.state.turn_status(busy))
            }
        }
    }

    pub async fn sync_status(&mut self) -> Result<()> {
        let new_status = self.derive_status();
        if new_status == self.state.status {
            return Ok(());
        }
        self.state.status = new_status.clone();
        self.parent
            .send(ParentEvent::StatusUpdate(new_status))
            .await?;
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn handle(
        &mut self,
        event: AgentEvent,
    ) -> Result<ControlFlow<()>> {
        #[allow(clippy::enum_glob_use)]
        use AgentEvent::*;

        debug!(event = ?event, "handling agent event");

        let result = match event {
            TaskDone(tid, result) => {
                self.handle_task_result(tid, result).await?;
                ControlFlow::Continue(())
            }
            TaskEvent(tid, generation, event) => {
                if self.tskmgr.pending(&tid) {
                    self.handle_history(generation, event).await?;
                }
                ControlFlow::Continue(())
            }
            External(event) => self.handle_external(event).await?,
        };
        self.sync_status().await?;
        Ok(result)
    }

    pub fn idle(&self) -> Result<()> {
        anyhow::ensure!(self.tskmgr.idle(), "agent is busy");
        Ok(())
    }

    async fn increment_generation(&mut self) -> Result<HistoryGeneration> {
        let generation = self.history().generation();
        self.handle_history(generation, HistoryUpdate::GenerationIncremented)
            .await?;
        Ok(self.history().generation())
    }

    async fn handle_external(
        &mut self,
        event: ExternalEvent,
    ) -> Result<ControlFlow<()>> {
        #[allow(clippy::enum_glob_use)]
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
                let event = if self.history().compacting() {
                    Some(HistoryUpdate::CompactAbort)
                } else if self
                    .history()
                    .state()
                    .status()
                    .is_some_and(|s| s.failable())
                {
                    Some(HistoryUpdate::TurnResponse(AssistantEvent::Failed(
                        ABORTED_BY_USER.into(),
                    )))
                } else {
                    None
                };
                if let Some(event) = event {
                    self.handle_history(g, event).await?;
                }
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
                self.handle_history(generation, history::HistoryUpdate::UserMessage(text))
                    .await?;
                self.increment_generation().await?;
                if multiplier <= 1 {
                    self.start_turn().await?;
                } else {
                    self.start_replica_turns(multiplier).await?;
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
                if self.history().compacting() {
                    self.compact_turn().await?;
                } else {
                    self.start_turn().await?;
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
        self.history_mut().handle(generation, event.clone())?;
        match event {
            HistoryUpdate::TurnResponse(AssistantEvent::Item(ref item)) => {
                self.execute_tool_calls(item).await?;
            }
            HistoryUpdate::TurnResponse(AssistantEvent::Failed(msg))
            | HistoryUpdate::CompactResponse(AssistantEvent::Failed(msg)) => {
                error!("response error in agent {}: {}", self.id, msg);
            }
            HistoryUpdate::GenerationIncremented
            | HistoryUpdate::TurnResponse(AssistantEvent::Delta(_))
            | HistoryUpdate::CompactResponse(AssistantEvent::Delta(_)) => return Ok(()),
            _ => {}
        }
        // TODO save less often; save on errors
        self.save().await?;
        Ok(())
    }

    async fn start_replica_turns(
        &mut self,
        n: usize,
    ) -> Result<()> {
        let handles: Vec<_> = stream::iter(0..n)
            .map(async |_| subagent::spawn(self, String::new(), true).await)
            .buffered(16)
            .try_collect()
            .await?;
        self.state
            .topology
            .children
            .extend(handles.iter().map(|handle| handle.id.clone()));
        self.save().await?;
        self.tskmgr.spawn(
            self.tx.clone(),
            self.history().generation(),
            move |task| async move {
                let created_at = now();
                let result = replica::run_replicas(handles).await?;
                task.send(HistoryUpdate::DeveloperMessage(DeveloperMessage::subagent(
                    result.report,
                    created_at,
                )))
                .await
            },
        );
        Ok(())
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
        let AssistantItem::ToolCall(call) = item else {
            return Ok(());
        };
        if call.task.output().is_some() {
            return Ok(());
        }
        let mut call = call.clone();
        let generation = self.history().generation();
        call.task.prepare(self).await?;
        self.tskmgr
            .spawn(self.tx.clone(), generation, move |task| async move {
                let handle = TurnHandle {
                    task,
                    turn_type: TurnType::Default,
                };
                call.task.run().await;
                call.touch_ready_at_now();
                handle
                    .send(AssistantEvent::Item(Box::new(AssistantItem::ToolCall(
                        call,
                    ))))
                    .await
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use futures::future::pending;
    use tokio::sync::mpsc::Receiver;
    use tokio::sync::mpsc::channel;
    use tokio::time::Duration;
    use tokio::time::timeout;

    use super::*;
    use crate::agent::AgentState;
    use crate::agent::init::channel_parent_sink;
    use crate::config::Config;
    use crate::llm::provider::assistant::Assistant;
    use crate::llm::provider::assistant::AssistantPool;
    use crate::project::Project;
    use crate::project::layout::LayoutTrait;

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
        let project = Project::new_test().unwrap();
        let aid = AgentId::from(format!("abort-test-{}", uuid::Uuid::new_v4()));
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
                topology: Default::default(),
                context: crate::agent::AgentContext {
                    commit: "".into(),
                    history: History::new("".into()),
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
            .handle(0, HistoryUpdate::TurnResponse(AssistantEvent::Created(0)))
            .unwrap();
        agent.tskmgr.spawn(agent.tx.clone(), 0, |_| async move {
            pending::<Result<()>>().await
        });

        let _ = agent.handle_external(ExternalEvent::Abort).await.unwrap();

        let events = [
            recv(&mut parent_rx, "parent event").await,
            recv(&mut parent_rx, "parent event").await,
            recv(&mut parent_rx, "parent event").await,
        ];
        assert!(matches!(
            events.as_slice(),
            [
                ParentEvent::HistoryUpdate(_, HistoryUpdate::GenerationIncremented),
                ParentEvent::HistoryUpdate(_, HistoryUpdate::TurnResponse(AssistantEvent::Failed(msg))),
                ParentEvent::StatusUpdate(crate::agent::AgentStatus::Normal(
                    crate::llm::history::TurnStatus::Failed(status),
                )),
            ] if msg == ABORTED_BY_USER && status == ABORTED_BY_USER
        ));
        assert!(matches!(
            agent.state.context.history.state().last(),
            Some(crate::llm::history::message::Message::Assistant(crate::llm::history::message::AssistantMessage {
                status: crate::llm::history::message::AssistantStatus::Error(msg),
                ..
            })) if msg == ABORTED_BY_USER
        ));

        tokio::fs::remove_dir_all(project.agent(&aid))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn retry_after_compact_failure_restarts_compaction() {
        let project = Project::new_test().unwrap();
        let aid = AgentId::from(format!("compact-retry-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(project.agent(&aid))
            .await
            .unwrap();
        let (parent_tx, _parent_rx) = channel(8);
        let (tx, rx) = channel(8);
        let assistant = assistant().await;
        let mut agent = Agent {
            project: project.clone(),
            id: aid.clone(),
            state: AgentState {
                status: Default::default(),
                assistant: assistant.clone(),
                topology: Default::default(),
                context: crate::agent::AgentContext {
                    commit: "".into(),
                    history: History::new("".into()),
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
                HistoryUpdate::DeveloperMessage(DeveloperMessage::misc("x".repeat(2000))),
            )
            .unwrap();
        history
            .handle(0, HistoryUpdate::CompactStart { n_drop: 1 })
            .unwrap();
        agent
            .handle_history(
                0,
                HistoryUpdate::CompactResponse(AssistantEvent::Failed("oops".into())),
            )
            .await
            .unwrap();

        let _ = agent.handle_external(ExternalEvent::Retry).await.unwrap();

        assert!(agent.state.context.history.compacting());

        tokio::fs::remove_dir_all(project.agent(&aid))
            .await
            .unwrap();
    }
}

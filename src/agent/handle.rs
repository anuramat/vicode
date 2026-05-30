use std::ops::ControlFlow;

use anyhow::Result;
use futures::StreamExt;
use futures::stream;
use tokio::sync::oneshot;
use tracing::debug;
use tracing::error;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentStatus;
use crate::agent::id::AgentId;
use crate::agent::router::SubagentSpawnSnapshot;
use crate::agent::subagent;
use crate::agent::subagent::replica;
use crate::agent::task::manager::TaskId;
use crate::agent::task::sink::TurnHandle;
use crate::agent::task::sink::TurnType;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::llm::history;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::TurnStatus;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::UserMessage;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::llm::provider::assistant::Assistant;
use crate::utils::now;

const ABORTED_BY_USER: &str = "aborted by user";

#[derive(Debug)]
pub enum AgentEvent {
    TaskDone(TaskId, Result<()>),
    TaskEvent(TaskId, HistoryGeneration, HistoryUpdate),
    External(ExternalEvent),
    /// Router asks for a snapshot of agent state to seed a hidden subagent.
    SnapshotRequest(oneshot::Sender<SubagentSpawnSnapshot>),
}

#[derive(Debug)]
pub enum ParentEvent {
    Started(Box<crate::agent::AgentState>),
    HistoryUpdate(HistoryGeneration, HistoryUpdate),
    StatusUpdate(AgentStatus),
    AssistantSet(Assistant),
    Error(String),
}

#[derive(Debug)]
pub enum ExternalEvent {
    /// Compact the first n messages
    Compact(usize),
    Retry,
    Abort,
    Undo(usize), // TODO maybe this should send generation or whatever
    SetAssistant(String),
    Submit(UserPrompt, Option<oneshot::Sender<TurnResult>>),
    DuplicateRequest(AgentId),
}

#[derive(Debug)]
pub enum TurnResult {
    Success { last_text: Option<String> },
    Failed(String),
}

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
        self.emit(ParentEvent::StatusUpdate(new_status)).await?;
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
            SnapshotRequest(reply) => {
                drop(reply.send(SubagentSpawnSnapshot {
                    commit: self.state.context.commit.clone(),
                    assistant_id: self.state.assistant.id.clone(),
                    history: self.state.context.history.clone(),
                    max_depth: self.state.max_depth,
                }));
                ControlFlow::Continue(())
            }
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
                let done = self.pending_done.take();
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
                    Some(HistoryUpdate::TurnResponse(AssistantEvent::failed(
                        ABORTED_BY_USER.into(),
                    )))
                } else {
                    None
                };
                if let Some(event) = event {
                    self.handle_history(g, event).await?;
                }
                if let Some(done) = done {
                    drop(done.send(TurnResult::Failed(ABORTED_BY_USER.into())));
                }
            }
            DuplicateRequest(aid) => {
                self.idle()?;
                self.try_duplicate(aid).await?;
            }
            Submit(prompt, done) => {
                self.start_submit(prompt, done).await?;
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

    async fn start_submit(
        &mut self,
        prompt: UserPrompt,
        done: Option<oneshot::Sender<TurnResult>>,
    ) -> Result<()> {
        self.idle()?;
        if let Some(prev) = self.pending_done.take() {
            drop(prev.send(TurnResult::Failed(ABORTED_BY_USER.into())));
        }
        self.pending_done = done;
        if let Err(e) = self.start_submit_inner(prompt).await {
            if let Some(done) = self.pending_done.take() {
                drop(done.send(TurnResult::Failed(e.to_string())));
            }
            return Err(e);
        }
        Ok(())
    }

    async fn start_submit_inner(
        &mut self,
        UserPrompt {
            text,
            multiplier,
            generation,
        }: UserPrompt,
    ) -> Result<()> {
        self.handle_history(
            generation,
            history::HistoryUpdate::UserMessage(UserMessage::new(text)),
        )
        .await?;
        self.increment_generation().await?;
        if multiplier <= 1 {
            self.start_turn().await?;
        } else {
            self.start_replica_turns(multiplier).await?;
        }
        Ok(())
    }

    pub async fn handle_history(
        &mut self,
        generation: HistoryGeneration,
        event: HistoryUpdate,
    ) -> Result<()> {
        self.history_mut().handle(generation, event.clone())?;
        self.emit(ParentEvent::HistoryUpdate(generation, event.clone()))
            .await?;
        match event {
            HistoryUpdate::TurnResponse(AssistantEvent::Item(ref item)) => {
                self.execute_tool_calls(item);
            }
            HistoryUpdate::TurnResponse(AssistantEvent::Failed { message, .. })
            | HistoryUpdate::CompactResponse(AssistantEvent::Failed { message, .. }) => {
                error!("response error in agent {}: {}", self.id, message);
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
        let results: Vec<_> = stream::iter(0..n)
            .map(async |_| {
                subagent::spawn_and_submit(
                    &self.router,
                    &self.project,
                    &self.id,
                    String::new(),
                    true,
                )
                .await
            })
            .buffered(16)
            .collect()
            .await;
        let mut handles = Vec::with_capacity(n);
        let mut spawn_errors = Vec::new();
        for r in results {
            match r {
                Ok(h) => handles.push(h),
                Err(e) => spawn_errors.push(e.to_string()),
            }
        }
        self.tskmgr.spawn(
            self.tx.clone(),
            self.history().generation(),
            move |task| async move {
                let created_at = now();
                let result = replica::run_replicas(handles, spawn_errors).await?;
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
        let new = ASSISTANT_POOL.get().unwrap().assistant(id)?;
        self.state.assistant = new.clone();
        self.save().await?;
        self.emit(ParentEvent::AssistantSet(new)).await?;
        Ok(())
    }

    pub fn execute_tool_calls(
        &mut self,
        item: &AssistantItem,
    ) {
        let AssistantItem::ToolCall(call) = item else {
            return;
        };
        if call.task.output().is_some() {
            return;
        }
        let mut call = call.clone();
        let generation = self.history().generation();
        let ctx =
            ToolRuntimeContext::new(self.id.clone(), self.project.clone(), self.router.clone());
        self.tskmgr
            .spawn(self.tx.clone(), generation, move |task| async move {
                let handle = TurnHandle {
                    task,
                    turn_type: TurnType::Default,
                };
                call.task.run(ctx).await;
                call.touch_ready_at_now();
                handle
                    .send(AssistantEvent::Item(Box::new(AssistantItem::ToolCall(
                        call,
                    ))))
                    .await
            });
    }

    /// Drive the pending oneshot for the current turn, if any.
    pub fn fire_pending_done(&mut self) {
        let Some(done) = self.pending_done.take() else {
            return;
        };
        let result = match self.derive_status() {
            AgentStatus::Normal(TurnStatus::Failed(msg))
            | AgentStatus::Compact(TurnStatus::Failed(msg)) => TurnResult::Failed(msg),
            _ => TurnResult::Success {
                last_text: self.history().state().last_text_output().ok(),
            },
        };
        drop(done.send(result));
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
    use crate::config::Config;
    use crate::llm::history::CompactStart;
    use crate::llm::history::History;
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
        let pool = crate::llm::provider::assistant::ASSISTANT_POOL
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
            .await;
        pool.assistant("test").unwrap()
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
            .handle(0, HistoryUpdate::TurnResponse(AssistantEvent::Created(0)))
            .unwrap();
        agent.tskmgr.spawn(agent.tx.clone(), 0, |_| async move {
            pending::<Result<()>>().await
        });
        let (done_tx, done_rx) = oneshot::channel();
        agent.pending_done = Some(done_tx);

        let _ = agent
            .handle(AgentEvent::External(ExternalEvent::Abort))
            .await
            .unwrap();

        let events = [
            parent_event(recv(&mut parent_rx, "parent event").await),
            parent_event(recv(&mut parent_rx, "parent event").await),
            parent_event(recv(&mut parent_rx, "parent event").await),
        ];
        assert!(matches!(
            events.as_slice(),
            [
                ParentEvent::HistoryUpdate(_, HistoryUpdate::GenerationIncremented),
                ParentEvent::HistoryUpdate(_, HistoryUpdate::TurnResponse(AssistantEvent::Failed { message: msg, .. })),
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
        assert!(matches!(
            timeout(RX_TIMEOUT, done_rx).await.unwrap().unwrap(),
            TurnResult::Failed(msg) if msg == ABORTED_BY_USER
        ));

        tokio::fs::remove_dir_all(project.agent(&aid))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn start_submit_failure_fires_pending_done_failed() {
        let project = Project::new_test().unwrap();
        let aid = AgentId::from(format!("submit-fail-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(project.agent(&aid))
            .await
            .unwrap();
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
            router: crate::agent::router::AgentRouter::test_handle(),
            pending_done: None,
            tx,
            rx,
            tskmgr: crate::agent::task::manager::AgentTaskManager::new(),
            tools: Default::default(),
        };

        let (done_tx, done_rx) = oneshot::channel();
        let stale_generation = agent.history().generation() + 1;
        let result = agent
            .handle(AgentEvent::External(ExternalEvent::Submit(
                UserPrompt {
                    text: "hi".into(),
                    multiplier: 1,
                    generation: stale_generation,
                },
                Some(done_tx),
            )))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            timeout(RX_TIMEOUT, done_rx).await.unwrap().unwrap(),
            TurnResult::Failed(_)
        ));
        assert!(agent.pending_done.is_none());

        tokio::fs::remove_dir_all(project.agent(&aid))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn set_assistant_emits_assistant_set_event() {
        let project = Project::new_test().unwrap();
        let aid = AgentId::from(format!("set-assistant-{}", uuid::Uuid::new_v4()));
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

        agent.set_assistant("test").await.unwrap();

        let event = parent_event(recv(&mut parent_rx, "parent event").await);
        assert!(
            matches!(event, ParentEvent::AssistantSet(ref a) if a.id == "test"),
            "{event:?}"
        );

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
            router: crate::agent::router::AgentRouter::test_handle(),
            pending_done: None,
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
            .handle(0, HistoryUpdate::CompactStart(CompactStart::new(1)))
            .unwrap();
        agent
            .handle_history(
                0,
                HistoryUpdate::CompactResponse(AssistantEvent::Created(0)),
            )
            .await
            .unwrap();
        agent
            .handle_history(
                0,
                HistoryUpdate::CompactResponse(AssistantEvent::failed("oops".into())),
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

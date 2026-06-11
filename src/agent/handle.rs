//! agent wire types and the effects interpreter: every event is translated
//! into a [`CoreEvent`], handed to the pure [`AgentCore`], and the resulting
//! effects are drained fully and in order — even when the core or an effect
//! fails, so the ledger and the TUI history mirror never desync.

use anyhow::Result;
use futures::StreamExt;
use futures::stream;
use tokio::sync::oneshot;
use tracing::debug;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentStatus;
use crate::agent::core::CoreEvent;
use crate::agent::core::Effect;
use crate::agent::id::AgentId;
use crate::agent::router::SubagentSpawnSnapshot;
use crate::agent::subagent;
use crate::agent::subagent::replica;
use crate::agent::task::ledger::TaskId;
use crate::agent::task::sink::TurnHandle;
use crate::agent::task::sink::TurnType;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::provider::assistant::Assistant;
use crate::utils::now;

#[derive(Debug)]
pub enum AgentEvent {
    TaskDone(TaskId, Result<()>),
    TaskEvent(TaskId, HistoryGeneration, HistoryUpdate),
    External(ExternalEvent),
    /// Router asks for a snapshot of agent state to seed a hidden subagent.
    SnapshotRequest(oneshot::Sender<SubagentSpawnSnapshot>),
}

#[derive(Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
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
#[cfg_attr(test, derive(serde::Serialize))]
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

/// IO payloads stripped from the incoming event, consumed by the drain
#[derive(Default)]
struct Staged {
    done: Option<oneshot::Sender<TurnResult>>,
    snapshot: Option<oneshot::Sender<SubagentSpawnSnapshot>>,
}

impl Agent {
    #[instrument(skip(self))]
    pub async fn handle(
        &mut self,
        event: AgentEvent,
    ) -> Result<()> {
        debug!(event = ?event, "handling agent event");
        let mut staged = Staged::default();
        let event = translate(event, &mut staged);
        let mut effects = Vec::new();
        let result = self.core.handle(now(), event, &mut effects);
        let mut drain_err = None;
        for effect in effects {
            if let Err(e) = self.interpret(effect, &mut staged).await {
                drain_err.get_or_insert(e);
            }
        }
        result.and(drain_err.map_or(Ok(()), Err))
    }

    async fn interpret(
        &mut self,
        effect: Effect,
        staged: &mut Staged,
    ) -> Result<()> {
        match effect {
            Effect::Emit(event) => self.emit(event).await?,
            Effect::Save => self.save().await?,
            Effect::StartTurn {
                id,
                generation,
                turn_type,
                assistant,
                tools,
                instructions,
                messages,
            } => {
                self.executor
                    .spawn(self.tx.clone(), id, generation, move |task| async move {
                        let handle = TurnHandle { task, turn_type };
                        if let Err(err) =
                            Self::turn(handle.clone(), &assistant, tools, instructions, messages)
                                .await
                        {
                            handle.send(AssistantEvent::failed(err.to_string())).await?;
                            return Err(err);
                        }
                        Ok(())
                    });
            }
            Effect::RunTool {
                id,
                generation,
                mut call,
            } => {
                let ctx = ToolRuntimeContext::new(
                    self.id.clone(),
                    self.project.clone(),
                    self.router.clone(),
                );
                self.executor
                    .spawn(self.tx.clone(), id, generation, move |task| async move {
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
            Effect::StartReplicas { id, generation, n } => {
                let router = self.router.clone();
                let project = self.project.clone();
                let aid = self.id.clone();
                self.executor
                    .spawn(self.tx.clone(), id, generation, move |task| async move {
                        // spawning inside the task keeps the parent loop free
                        // to answer the snapshot requests this triggers
                        let results: Vec<_> = stream::iter(0..n)
                            .map(|_| {
                                subagent::spawn_and_submit(
                                    &router,
                                    &project,
                                    &aid,
                                    String::new(),
                                    true,
                                )
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
                        let created_at = now();
                        let result = replica::run_replicas(handles, spawn_errors).await?;
                        task.send(HistoryUpdate::DeveloperMessage(DeveloperMessage::subagent(
                            result.report,
                            created_at,
                        )))
                        .await
                    });
            }
            Effect::AbortTasks => self.executor.shutdown().await,
            Effect::SetAssistant(new) => {
                // state applied iff persisted: the TUI never sees an unsaved assistant
                let mut state = self.core.state.clone();
                state.assistant = new.clone();
                state.save(&self.project, &self.id).await?;
                self.core.state.assistant = new.clone();
                self.emit(ParentEvent::AssistantSet(new)).await?;
            }
            Effect::ReplyDone(result) => {
                if let Some(done) = self.pending_done.take() {
                    drop(done.send(result));
                }
            }
            Effect::StoreDone => self.pending_done = staged.done.take(),
            Effect::ReplySnapshot(snap) => {
                if let Some(reply) = staged.snapshot.take() {
                    drop(reply.send(snap));
                }
            }
            Effect::Duplicate(aid) => self.try_duplicate(aid).await?,
        }
        Ok(())
    }
}

fn translate(
    event: AgentEvent,
    staged: &mut Staged,
) -> CoreEvent {
    match event {
        AgentEvent::TaskDone(tid, result) => {
            CoreEvent::TaskDone(tid, result.map_err(|e| e.to_string()))
        }
        AgentEvent::TaskEvent(tid, generation, update) => {
            CoreEvent::TaskEvent(tid, generation, update)
        }
        AgentEvent::SnapshotRequest(reply) => {
            staged.snapshot = Some(reply);
            CoreEvent::Snapshot
        }
        AgentEvent::External(event) => match event {
            ExternalEvent::Submit(prompt, done) => {
                let has_done = done.is_some();
                staged.done = done;
                CoreEvent::Submit(prompt, has_done)
            }
            ExternalEvent::Compact(n) => CoreEvent::Compact(n),
            ExternalEvent::Retry => CoreEvent::Retry,
            ExternalEvent::Abort => CoreEvent::Abort,
            ExternalEvent::Undo(n) => CoreEvent::Undo(n),
            ExternalEvent::SetAssistant(id) => CoreEvent::SetAssistant(id),
            ExternalEvent::DuplicateRequest(aid) => CoreEvent::Duplicate(aid),
        },
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::Receiver;
    use tokio::time::Duration;
    use tokio::time::timeout;

    use super::*;
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

    #[tokio::test]
    async fn submit_failure_fires_staged_done_failed() {
        let (mut agent, _api, _parent_rx) = Agent::fake("submit-fail").await;

        let (done_tx, done_rx) = oneshot::channel();
        let stale_generation = agent.core.history().generation() + 1;
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
        assert!(!agent.core.pending_done);
    }

    #[tokio::test]
    async fn set_assistant_switches_and_emits() {
        let (mut agent, _api, mut parent_rx) = Agent::fake("set-assistant").await;

        agent
            .handle(AgentEvent::External(ExternalEvent::SetAssistant(
                "test2".into(),
            )))
            .await
            .unwrap();

        let event = parent_event(recv(&mut parent_rx, "parent event").await);
        assert!(
            matches!(event, ParentEvent::AssistantSet(ref a) if a.id == "test2"),
            "{event:?}"
        );
        assert_eq!(agent.core.state.assistant.id, "test2");
    }
}

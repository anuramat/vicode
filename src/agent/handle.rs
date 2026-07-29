//! agent wire types and the effects interpreter: every event is translated
//! into a [`CoreEvent`], handed to the pure [`AgentCore`], and the resulting
//! effects are drained fully and in order — even when the core or an effect
//! fails, so the ledger and the TUI history mirror never desync.

use std::panic::AssertUnwindSafe;

use anyhow::Result;
use anyhow::anyhow;
use futures::FutureExt;
use tracing::debug;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::ActivityStatus;
use crate::agent::core::CoreEvent;
use crate::agent::core::Effect;
use crate::agent::id::AgentId;
use crate::agent::router::api::TurnOutcome;
use crate::agent::router::graph::NodeStatus;
use crate::agent::router::graph::StatusPing;
use crate::agent::task::ledger::TaskId;
use crate::agent::task::sink::OutputSink;
use crate::agent::task::sink::TaskHandle;
use crate::agent::task::sink::TurnHandle;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::TurnStatus;
use crate::llm::history::message::ToolCallItem;
use crate::llm::history::message::UserMessage;
use crate::utils::now;

#[derive(Debug)]
pub enum AgentEvent {
    TaskDone(TaskId, Result<()>),
    TaskEvent(TaskId, HistoryGeneration, HistoryUpdate),
    /// reaper: tool future returned its resolved item
    ToolResolved(TaskId, Box<ToolCallItem>),
    /// reaper: tool future panicked/cancelled; error = marker + partial output
    ToolFailed {
        id: TaskId,
        call_id: String,
        error: String,
    },
    /// inter-agent message (router `send` / spawn seed): wakes a turn if
    /// idle, buffers if busy (§2.2)
    Inbound(UserMessage),
    External(ExternalEvent),
}

#[derive(Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum ParentEvent {
    Started(Box<crate::agent::AgentState>),
    HistoryUpdate(HistoryGeneration, HistoryUpdate),
    StatusUpdate(ActivityStatus),
    AssistantSet(String),
    Error(String),
    /// live tool-output chunk for rendering; the `Agent` accumulator is
    /// authoritative, so a dropped chunk costs a render frame only
    ToolOutput {
        call_id: String,
        chunk: String,
    },
}

#[derive(Debug)]
pub enum ExternalEvent {
    /// Compact the first n messages
    Compact(usize),
    Retry,
    Abort,
    Undo(usize), // TODO maybe this should send generation or whatever
    SetAssistant(String),
    Submit(UserPrompt),
    DuplicateRequest {
        copy: AgentId,
        /// resolved iff the copy registered; dropped-channel semantics make
        /// the failure signal total — busy rejection, unknown aid, dead
        /// `Agent`, and panic all close the app's receiver (M5)
        ack: tokio::sync::oneshot::Sender<()>,
    },
}

#[derive(Debug)]
pub struct UserPrompt {
    pub text: String,
    /// None = the receiving agent uses its current one
    pub generation: Option<HistoryGeneration>,
}

impl Agent {
    #[instrument(skip(self))]
    pub async fn handle(
        &mut self,
        event: AgentEvent,
    ) -> Result<()> {
        debug!(event = ?event, "handling agent event");
        let event = translate(event, &mut self.dup_ack);
        let mut effects = Vec::new();
        let result = self.core.handle(now(), event, &mut effects);
        let mut drain_err = None;
        for effect in effects {
            if let Err(e) = self.interpret(effect).await {
                drain_err.get_or_insert(e);
            }
        }
        // an ack the drain didn't consume drops here: the core's busy
        // rejection and a failed duplicate alike resolve the app's receiver
        // as an error (M5)
        self.dup_ack = None;
        result.and(drain_err.map_or(Ok(()), Err))
    }

    /// startup wake: run the core's resume (flush mail buffered before a
    /// restart, start its turn) and drain the effects (M3)
    pub async fn resume(&mut self) -> Result<()> {
        let mut effects = Vec::new();
        self.core.resume(now(), &mut effects)?;
        for effect in effects {
            self.interpret(effect).await?;
        }
        Ok(())
    }

    async fn interpret(
        &mut self,
        effect: Effect,
    ) -> Result<()> {
        match effect {
            Effect::Emit(event) => {
                // double-send: liveness to the router, rendering to the app —
                // so rendering stays independent of the router loop
                if matches!(event, ParentEvent::StatusUpdate(_)) {
                    self.ping_status(None).await?;
                }
                self.emit(event).await?;
            }
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
                let task = TaskHandle::new(id, generation, self.tx.clone());
                self.executor.spawn_turn(id, async move {
                    let handle = TurnHandle { task, turn_type };
                    // catch a panic so a panicked turn still lands its terminal
                    // Failed event — otherwise the assistant message stays
                    // InProgress forever and the next flush stacks a turn on
                    // the orphan; a panic thus becomes the same clean Err path
                    // as any turn failure (H6, §2.4a)
                    let outcome = AssertUnwindSafe(Self::turn(
                        handle.clone(),
                        &assistant,
                        tools,
                        instructions,
                        messages,
                    ))
                    .catch_unwind()
                    .await;
                    let result = outcome.unwrap_or_else(|panic| {
                        Err(anyhow!(
                            "turn panicked: {}",
                            crate::agent::run::panic_message(&*panic)
                        ))
                    });
                    if let Err(err) = result {
                        handle.send(AssistantEvent::failed(err.to_string())).await?;
                        return Err(err);
                    }
                    Ok(())
                });
            }
            Effect::RunTool {
                id,
                mut call,
                capture,
            } => {
                let ctx = ToolRuntimeContext::new(
                    self.id.clone(),
                    self.project.clone(),
                    self.router.clone(),
                    OutputSink::new(call.call_id.clone(), self.out_tx.clone()),
                    capture,
                );
                self.executor
                    .spawn_tool(id, call.call_id.clone(), async move {
                        call.task.run(ctx).await;
                        call.touch_ready_at_now();
                        call
                    });
            }
            Effect::AbortTasks => {
                // hard-abort, then reap synchronously: every in-flight call
                // is finalized (partial output retained) before abort returns
                self.executor.abort_all();
                while let Some((meta, output)) = self.executor.reap().await {
                    if let Some(event) = self.reap_event(meta, output) {
                        Box::pin(self.handle(event)).await?;
                    }
                }
            }
            Effect::SetAssistant(new) => {
                // state applied iff persisted: the TUI never sees an unsaved assistant
                let mut state = self.core.state.clone();
                state.assistant = new.clone();
                state.save(&self.project, &self.id).await?;
                self.core.state.assistant = new.clone();
                self.emit(ParentEvent::AssistantSet(new)).await?;
            }
            Effect::Duplicate(aid) => {
                self.try_duplicate(aid).await?;
                // ack only a registered copy; the copy's own Started event
                // wires the app's preview tab (M5)
                if let Some(ack) = self.dup_ack.take() {
                    let _ = ack.send(());
                }
            }
        }
        Ok(())
    }

    /// the `Agent`→router liveness ping carrying the processed-delivery
    /// watermark (H4/M2); the idle ping carries the last
    /// assistant text (`wait`'s return), a failed turn its typed error
    /// instead — the output cache is never clobbered by a failure (M1).
    /// `error` is a handler failure from the run loop (a wake that couldn't
    /// start its turn), surfaced the same way.
    pub async fn ping_status(
        &self,
        error: Option<String>,
    ) -> Result<()> {
        let (status, output, turn_error) = match self.core.state.status.turn() {
            TurnStatus::InProgress => (NodeStatus::Running, None, None),
            TurnStatus::Idle => (
                NodeStatus::Idle,
                self.core.history().state().last_text_output().ok(),
                None,
            ),
            TurnStatus::Failed(msg) => (NodeStatus::Idle, None, Some(msg.clone())),
        };
        let ping = StatusPing {
            processed: self.processed,
            status,
            outcome: TurnOutcome {
                output,
                error: error.or(turn_error),
            },
        };
        self.router.status(self.id.clone(), ping).await
    }
}

/// oneshots are stripped here: the ack parks in the `Agent`'s slot, so the
/// core stays channel-free (M5)
fn translate(
    event: AgentEvent,
    dup_ack: &mut Option<tokio::sync::oneshot::Sender<()>>,
) -> CoreEvent {
    match event {
        AgentEvent::TaskDone(tid, result) => {
            CoreEvent::TaskDone(tid, result.map_err(|e| e.to_string()))
        }
        AgentEvent::TaskEvent(tid, generation, update) => {
            CoreEvent::TaskEvent(tid, generation, update)
        }
        AgentEvent::ToolResolved(tid, item) => CoreEvent::ToolResolved(tid, item),
        AgentEvent::ToolFailed { id, call_id, error } => {
            CoreEvent::ToolFailed { id, call_id, error }
        }
        AgentEvent::Inbound(msg) => CoreEvent::Message(msg),
        AgentEvent::External(event) => match event {
            ExternalEvent::Submit(prompt) => CoreEvent::Submit(prompt),
            ExternalEvent::Compact(n) => CoreEvent::Compact(n),
            ExternalEvent::Retry => CoreEvent::Retry,
            ExternalEvent::Abort => CoreEvent::Abort,
            ExternalEvent::Undo(n) => CoreEvent::Undo(n),
            ExternalEvent::SetAssistant(id) => CoreEvent::SetAssistant(id),
            ExternalEvent::DuplicateRequest { copy, ack } => {
                *dup_ack = Some(ack);
                CoreEvent::Duplicate(copy)
            }
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
    async fn submit_with_stale_generation_errors() {
        let (mut agent, _api, _parent_rx) = Agent::fake("submit-fail").await;

        let stale_generation = agent.core.history().generation() + 1;
        let result = agent
            .handle(AgentEvent::External(ExternalEvent::Submit(UserPrompt {
                text: "hi".into(),
                generation: Some(stale_generation),
            })))
            .await;
        assert!(result.is_err());
    }

    /// M5: a rejected duplicate (busy original) drops the ack — the app's
    /// receiver resolves as an error and the preview tab rolls back; the old
    /// code returned the error with no signal at all
    #[tokio::test]
    async fn duplicate_request_while_busy_drops_the_ack() {
        let (mut agent, _api, _parent_rx) = Agent::fake("dup-busy").await;
        agent.core.ledger.register();

        let (ack, ack_rx) = tokio::sync::oneshot::channel();
        let result = agent
            .handle(AgentEvent::External(ExternalEvent::DuplicateRequest {
                copy: crate::agent::id::AgentId::from("copy".to_string()),
                ack,
            }))
            .await;

        assert!(result.is_err());
        assert!(ack_rx.await.is_err(), "ack must drop, not fire");
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
            matches!(event, ParentEvent::AssistantSet(ref a) if a == "test2"),
            "{event:?}"
        );
        assert_eq!(agent.core.state.assistant, "test2");
    }
}

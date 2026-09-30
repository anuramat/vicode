//! the effects interpreter: every event is handed to the pure
//! [`AgentCore`](crate::agent::core::AgentCore), and the resulting effects
//! are drained fully and in order — even when the core or an effect fails,
//! so the ledger and the TUI history mirror never desync.

use anyhow::Result;
use tracing::debug;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::core::Effect;
use crate::agent::event::AgentEvent;
use crate::agent::event::UiEvent;
use crate::agent::event::UserCommand;
use crate::agent::router::api::TurnOutcome;
use crate::agent::router::graph::NodeStatus;
use crate::agent::router::graph::StatusReport;
use crate::agent::task::sink::TaskSink;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::llm::history::TurnStatus;
use crate::tui::app::AppEvent;
use crate::utils::now;

impl Agent {
    #[instrument(skip(self))]
    pub async fn handle(
        &mut self,
        event: AgentEvent,
    ) -> Result<()> {
        debug!(event = ?event, "handling agent event");
        // an abort keeps everything its tools streamed before it: apply the
        // queued output first (queued turn events die with the turn anyway)
        if matches!(event, AgentEvent::User(UserCommand::Abort)) {
            for _ in 0..self.task_rx.len() {
                if let Ok(output @ AgentEvent::Output(..)) = self.task_rx.try_recv() {
                    self.apply(output).await?;
                }
            }
        }
        self.apply(event).await
    }

    async fn apply(
        &mut self,
        event: AgentEvent,
    ) -> Result<()> {
        let mut effects = Vec::new();
        let result = self.core.handle(now(), event, &mut effects);
        result.and(self.interpret_all(effects).await)
    }

    /// startup wake: run the core's resume (flush mail buffered before a
    /// restart, start its turn) and drain the effects
    pub async fn resume(&mut self) -> Result<()> {
        let mut effects = Vec::new();
        self.core.resume(now(), &mut effects)?;
        self.interpret_all(effects).await
    }

    /// interpret every effect, even past a failure; the first error wins
    async fn interpret_all(
        &mut self,
        effects: Vec<Effect>,
    ) -> Result<()> {
        let mut result = Ok(());
        for effect in effects {
            let outcome = self.interpret(effect).await;
            if result.is_ok() {
                result = outcome;
            }
        }
        result
    }

    async fn interpret(
        &mut self,
        effect: Effect,
    ) -> Result<()> {
        match effect {
            // droppable: a full app bus costs render frames, never stalls us
            Effect::Ui(event @ UiEvent::ToolOutput { .. }) => {
                drop(
                    self.app_tx
                        .try_send(AppEvent::Agent(self.id.clone(), event)),
                );
            }
            Effect::Ui(event) => self.emit(event).await?,
            // liveness to the router, rendering to the app
            Effect::Status(status) => {
                self.report_status(None);
                self.emit(UiEvent::StatusUpdate(status)).await?;
            }
            Effect::Save => self.save().await?,
            Effect::StartTurn {
                id,
                assistant,
                tools,
                instructions,
                messages,
            } => {
                let sink = TaskSink::new(id, self.task_tx.clone());
                self.executor.spawn(id, "turn", async move {
                    Self::turn(sink, &assistant, tools, instructions, messages)
                        .await
                        .map(|()| None)
                        .map_err(|e| e.to_string())
                });
            }
            Effect::RunTool {
                id,
                mut call,
                inherited_history,
            } => {
                let ctx = ToolRuntimeContext::new(
                    self.id.clone(),
                    self.project.clone(),
                    self.router.clone(),
                    TaskSink::new(id, self.task_tx.clone()),
                    inherited_history,
                );
                self.executor.spawn(id, "tool", async move {
                    call.task.run(ctx).await;
                    call.touch_ready_at_now();
                    Ok(Some(Box::new(call)))
                });
            }
            Effect::AbortTasks => self.executor.abort_all(),
            Effect::SetAssistant(new) => {
                // state applied iff persisted: the TUI never sees an unsaved assistant
                let mut state = self.core.state.clone();
                state.assistant_id = new.clone();
                state.save(&self.project, &self.id).await?;
                self.core.state.assistant_id = new.clone();
                self.emit(UiEvent::AssistantSet(new)).await?;
            }
            // success needs no reply: the copy's own Started event attaches
            // the app's preview tab
            Effect::Duplicate(copy) => {
                if let Err(e) = self.try_duplicate(copy.clone()).await {
                    let error = format!("{e:#}");
                    self.emit(UiEvent::DuplicateFailed { copy, error }).await?;
                }
            }
        }
        Ok(())
    }

    /// the `Agent`→router status report carrying the processed-delivery
    /// watermark; the idle report carries the last
    /// assistant text (`wait`'s return), a failed turn its typed error
    /// instead — the output cache is never clobbered by a failure.
    /// `error` is a handler failure from the run loop (a wake that couldn't
    /// start its turn), surfaced the same way.
    pub fn report_status(
        &self,
        error: Option<String>,
    ) {
        let (status, output, turn_error) = match self.core.state.status.turn() {
            TurnStatus::InProgress => (NodeStatus::Running, None, None),
            TurnStatus::Idle => (
                NodeStatus::Idle,
                self.core.history().state().last_text_output().ok(),
                None,
            ),
            TurnStatus::Failed(msg) => (NodeStatus::Idle, None, Some(msg.clone())),
        };
        let report = StatusReport {
            processed: self.processed,
            status,
            outcome: TurnOutcome {
                output,
                error: error.or(turn_error),
            },
        };
        self.router.status(&self.id, report);
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::Receiver;
    use tokio::time::Duration;
    use tokio::time::timeout;

    use super::*;
    use crate::agent::event::UserPrompt;
    use crate::agent::task::ledger::Task;

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

    fn ui_event(event: AppEvent) -> UiEvent {
        match event {
            AppEvent::Agent(_, event) => event,
            other => panic!("expected an agent event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_with_stale_generation_errors() {
        let (mut agent, _api, _parent_rx) = Agent::fake("submit-fail").await;

        let stale_generation = agent.core.history().generation() + 1;
        let result = agent
            .handle(AgentEvent::User(UserCommand::Submit(UserPrompt {
                text: "hi".into(),
                generation: Some(stale_generation),
            })))
            .await;
        assert!(result.is_err());
    }

    /// a rejected duplicate (busy original) reports back naming the copy,
    /// so the app can roll its preview tab back
    #[tokio::test]
    async fn duplicate_while_busy_reports_failure_for_the_copy() {
        let (mut agent, _api, mut parent_rx) = Agent::fake("dup-busy").await;
        agent.core.ledger.register(Task::turn());

        let copy = crate::agent::id::AgentId::from("copy".to_string());
        agent
            .handle(AgentEvent::User(UserCommand::Duplicate(copy.clone())))
            .await
            .unwrap();

        let event = ui_event(recv(&mut parent_rx, "ui event").await);
        assert!(
            matches!(&event, UiEvent::DuplicateFailed { copy: c, error } if *c == copy && error == "agent is busy"),
            "{event:?}"
        );
    }

    #[tokio::test]
    async fn set_assistant_switches_and_emits() {
        let (mut agent, _api, mut parent_rx) = Agent::fake("set-assistant").await;

        agent
            .handle(AgentEvent::User(UserCommand::SetAssistant("test2".into())))
            .await
            .unwrap();

        let event = ui_event(recv(&mut parent_rx, "ui event").await);
        assert!(
            matches!(event, UiEvent::AssistantSet(ref a) if a == "test2"),
            "{event:?}"
        );
        assert_eq!(agent.core.state.assistant_id, "test2");
    }
}

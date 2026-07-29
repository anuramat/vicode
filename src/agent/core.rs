//! pure agent decision logic: no IO, no clocks, no channels, no tokio. The
//! `Agent` (`handle.rs`) translates wire events into [`CoreEvent`]s, calls
//! [`AgentCore::handle`], and interprets the produced [`Effect`]s in order.

use std::sync::Arc;

use anyhow::Result;

use crate::agent::AgentState;
use crate::agent::ActivityStatus;
use crate::agent::handle::ParentEvent;
use crate::agent::handle::UserPrompt;
use crate::agent::id::AgentId;
use crate::agent::task::ledger::TaskId;
use crate::agent::task::ledger::TaskLedger;
use crate::agent::task::sink::TurnType;
use crate::agent::tool::registry::TOOL_REGISTRY;
use crate::agent::tool::registry::ToolRegistry;
use crate::forward;
use crate::llm::history::Activity;
use crate::llm::history::AssistantEvent;
use crate::llm::history::CompactStart;
use crate::llm::history::History;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::Message;
use crate::llm::history::message::ToolCallItem;
use crate::llm::history::message::UserMessage;
use crate::llm::provider::assistant::Assistant;
use crate::llm::provider::assistant::AssistantPool;

pub const ABORTED_BY_USER: &str = "aborted by user";
pub const LOST_ON_RESTART: &str = "lost on restart";

#[derive(Debug)]
pub struct AgentCore {
    pub state: AgentState,
    pub ledger: TaskLedger,
    pub tools: ToolRegistry,
    pub assistants: Arc<AssistantPool>,
}

/// `AgentEvent` minus IO payloads: oneshots are stripped by `Agent`
#[derive(Debug)]
pub enum CoreEvent {
    TaskDone(TaskId, Result<(), String>),
    TaskEvent(TaskId, HistoryGeneration, HistoryUpdate),
    /// reaper: a tool future returned its resolved item
    ToolResolved(TaskId, Box<ToolCallItem>),
    /// reaper: a tool future panicked or was cancelled; `error` carries the
    /// marker plus whatever output `Agent` accumulated before the drop
    ToolFailed {
        id: TaskId,
        call_id: String,
        error: String,
    },
    /// inbound inter-agent message: wake if idle, buffer if busy — never
    /// mid-stream, never mid-compact (§2.2)
    Message(UserMessage),
    Submit(UserPrompt),
    Compact(usize),
    Retry,
    Abort,
    Undo(usize),
    SetAssistant(String),
    Duplicate(AgentId),
}

/// fat effects: all decision-time state is captured at push time, so the
/// `Agent` interprets them blindly and strictly in order
#[derive(Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum Effect {
    Emit(ParentEvent),
    /// marker; `Agent` serializes the core state as of this position
    Save,
    StartTurn {
        id: TaskId,
        generation: HistoryGeneration,
        turn_type: TurnType,
        assistant: Assistant,
        #[cfg_attr(test, serde(skip))]
        tools: ToolRegistry,
        #[cfg_attr(test, serde(skip))]
        instructions: String,
        #[cfg_attr(test, serde(skip))]
        messages: Vec<Message>,
    },
    RunTool {
        id: TaskId,
        call: ToolCallItem,
        /// `spawn` only: the parent's live history, snapshotted at dispatch
        /// via the `spawn_capture` hook (§2.2)
        #[cfg_attr(test, serde(skip))]
        capture: Option<History>,
    },
    AbortTasks,
    /// `Agent`: save state-with-new; on success apply + emit `AssistantSet`
    SetAssistant(String),
    Duplicate(AgentId),
}

impl AgentCore {
    forward! {
        history: History = self.state.context.history;
    }

    pub fn new(
        mut state: AgentState,
        assistants: Arc<AssistantPool>,
    ) -> Self {
        // restore repair: a dangling function_call in history would 400
        // every later turn
        state
            .context
            .history
            .fail_unresolved_tool_calls(LOST_ON_RESTART);
        Self {
            state,
            ledger: TaskLedger::default(),
            tools: TOOL_REGISTRY.clone(),
            assistants,
        }
    }

    pub fn handle(
        &mut self,
        now: u64,
        event: CoreEvent,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        #[allow(clippy::enum_glob_use)]
        use CoreEvent::*;

        let result = match event {
            TaskDone(tid, result) => self.task_done(now, tid, result, effects),
            TaskEvent(tid, generation, event) => {
                if self.ledger.pending(&tid) {
                    self.handle_history(generation, event, effects)
                } else {
                    Ok(())
                }
            }
            // reaper terminals resolve at the current generation and past a
            // cleared ledger: the slot must land its output even post-abort
            ToolResolved(tid, item) => {
                let g = self.history().generation();
                self.handle_history(
                    g,
                    HistoryUpdate::TurnResponse(AssistantEvent::Item(Box::new(
                        AssistantItem::ToolCall(*item),
                    ))),
                    effects,
                )?;
                self.task_finished(now, tid, effects)
            }
            ToolFailed { id, call_id, error } => {
                let g = self.history().generation();
                self.handle_history(g, HistoryUpdate::ToolCallFailed { call_id, error }, effects)?;
                self.task_finished(now, id, effects)
            }
            Message(msg) => self.message(now, msg, effects),
            Submit(prompt) => self.submit(now, prompt, effects),
            Compact(n) => {
                self.idle()?;
                self.init_compact(now, n, effects)?;
                self.compact_turn(now, effects)
            }
            Retry => {
                self.idle()?;
                self.increment_generation(effects)?;
                if self.history().compacting() {
                    self.compact_turn(now, effects)
                } else {
                    self.start_turn(now, effects)
                }
            }
            Abort => self.abort(now, effects),
            Undo(n) => {
                self.idle()?;
                let g = self.increment_generation(effects)?;
                self.handle_history(g, HistoryUpdate::Pop(n), effects)
            }
            SetAssistant(id) => {
                self.idle()?;
                let new = self.assistants.assistant(&id)?;
                effects.push(Effect::SetAssistant(new.id));
                Ok(())
            }
            Duplicate(aid) => {
                self.idle()?;
                effects.push(Effect::Duplicate(aid));
                Ok(())
            }
        };
        if result.is_ok() {
            self.sync_status(effects);
        }
        result
    }

    pub fn derive_status(&self) -> ActivityStatus {
        let busy = !self.ledger.idle();
        match self.history().activity() {
            Activity::Normal { state } => ActivityStatus::Normal(state.turn_status(busy)),
            Activity::Compacting { compact, .. } => {
                ActivityStatus::Compact(compact.state.turn_status(busy))
            }
        }
    }

    fn sync_status(
        &mut self,
        effects: &mut Vec<Effect>,
    ) {
        let new_status = self.derive_status();
        if new_status == self.state.status {
            return;
        }
        self.state.status = new_status.clone();
        effects.push(Effect::Emit(ParentEvent::StatusUpdate(new_status)));
    }

    pub fn idle(&self) -> Result<()> {
        anyhow::ensure!(self.ledger.idle(), "agent is busy");
        Ok(())
    }

    fn handle_history(
        &mut self,
        generation: HistoryGeneration,
        event: HistoryUpdate,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        // one clone: the match borrows, then the event moves into the Emit —
        // payloads (resolved tool calls) can embed a full workdir diff
        self.history_mut().handle(generation, event.clone())?;
        match &event {
            HistoryUpdate::TurnResponse(AssistantEvent::Item(item)) => {
                self.run_tool_call(item, effects)?;
            }
            HistoryUpdate::TurnResponse(AssistantEvent::Failed { message, .. })
            | HistoryUpdate::CompactResponse(AssistantEvent::Failed { message, .. }) => {
                tracing::error!("response error: {message}");
            }
            _ => {}
        }
        let skip_save = matches!(
            event,
            HistoryUpdate::GenerationIncremented
                | HistoryUpdate::TurnResponse(AssistantEvent::Delta(_))
                | HistoryUpdate::CompactResponse(AssistantEvent::Delta(_))
        );
        effects.push(Effect::Emit(ParentEvent::HistoryUpdate(generation, event)));
        if skip_save {
            return Ok(());
        }
        // TODO save less often; save on errors
        // every Save in a drain serializes the same post-handle state, so one
        // per drain suffices
        if !effects.iter().any(|e| matches!(e, Effect::Save)) {
            effects.push(Effect::Save);
        }
        Ok(())
    }

    fn run_tool_call(
        &mut self,
        item: &AssistantItem,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        let AssistantItem::ToolCall(call) = item else {
            return Ok(());
        };
        // also the recursion terminator: resolved calls re-enter
        // handle_history with the output already set
        if call.task.output().is_some() {
            return Ok(());
        }
        // the one capture hook (§2.2): the core is the only holder of the
        // live history, so `spawn` snapshots it here, at dispatch
        let capture = call
            .task
            .spawn_capture()
            .map(|inherit| self.history().subagent(inherit));
        effects.push(Effect::RunTool {
            id: self.ledger.register(),
            call: call.clone(),
            capture,
        });
        Ok(())
    }

    fn increment_generation(
        &mut self,
        effects: &mut Vec<Effect>,
    ) -> Result<HistoryGeneration> {
        let generation = self.history().generation();
        self.handle_history(generation, HistoryUpdate::GenerationIncremented, effects)?;
        Ok(self.history().generation())
    }

    fn task_done(
        &mut self,
        now: u64,
        id: TaskId,
        result: Result<(), String>,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        // stale failures still surface: emit before the ledger check
        if let Err(err) = result {
            effects.push(Effect::Emit(ParentEvent::Error(err)));
        }
        self.task_finished(now, id, effects)
    }

    /// shared continuation for everything that finishes a ledger task
    fn task_finished(
        &mut self,
        now: u64,
        id: TaskId,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        if !self.ledger.finish(&id) || !self.ledger.idle() {
            return Ok(());
        }
        // flush at the true idle; a flushed message wakes on its own
        let flushed = self.flush_pending(effects)?;
        if (flushed || self.history().state().needs_another_turn()) && !self.history().compacting()
        {
            self.start_turn(now, effects)
        } else {
            Ok(())
        }
    }

    /// the single inbound delivery path (§2.2)
    fn message(
        &mut self,
        now: u64,
        msg: UserMessage,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        self.state.pending_messages.push(msg);
        if self.ledger.idle() && !self.history().compacting() {
            self.flush_pending(effects)?;
            self.start_turn(now, effects)
        } else {
            // buffered and saved: survives restart and hibernation
            effects.push(Effect::Save);
            Ok(())
        }
    }

    /// startup wake (§2.2): a restored idle agent flushes messages buffered
    /// before the restart/hibernation and starts their turn — otherwise they
    /// sit unread until an unrelated event pokes the agent, and a post-restart
    /// `wait` fires on the pre-message output (M3)
    pub fn resume(
        &mut self,
        now: u64,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        if !self.ledger.idle() || self.history().compacting() {
            return Ok(());
        }
        if self.flush_pending(effects)? {
            self.start_turn(now, effects)?;
            self.sync_status(effects);
        }
        Ok(())
    }

    /// deliver buffered inbound messages at the current generation; never
    /// mid-compact (a compacting history rejects user messages)
    fn flush_pending(
        &mut self,
        effects: &mut Vec<Effect>,
    ) -> Result<bool> {
        if self.history().compacting() {
            return Ok(false);
        }
        let pending = std::mem::take(&mut self.state.pending_messages);
        let flushed = !pending.is_empty();
        for msg in pending {
            let generation = self.history().generation();
            self.handle_history(generation, HistoryUpdate::UserMessage(msg), effects)?;
        }
        Ok(flushed)
    }

    fn submit(
        &mut self,
        now: u64,
        UserPrompt { text, generation }: UserPrompt,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        // busy/compacting: queue instead of reject — pending messages carry
        // no generation and flush into a fresh turn at the next true idle, so
        // typed input is never destroyed and doubles as steering (M8)
        if !self.ledger.idle() || self.history().compacting() {
            self.state
                .pending_messages
                .push(UserMessage::new(text, now));
            effects.push(Effect::Save);
            return Ok(());
        }
        let generation = generation.unwrap_or_else(|| self.history().generation());
        // a stale submit is rejected *before* the flush: pending messages
        // carry no generation and must never be dropped as stale
        anyhow::ensure!(
            generation == self.history().generation(),
            "history generation mismatch: expected {}",
            self.history().generation(),
        );
        // drains a buffer stranded by abort — abort itself never flushes:
        // a naive flush would auto-start a turn on user abort
        self.flush_pending(effects)?;
        self.handle_history(
            generation,
            HistoryUpdate::UserMessage(UserMessage::new(text, now)),
            effects,
        )?;
        self.increment_generation(effects)?;
        self.start_turn(now, effects)
    }

    fn abort(
        &mut self,
        now: u64,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        self.ledger.clear();
        let g = self.increment_generation(effects)?;
        let event = if self.history().compacting() {
            Some(HistoryUpdate::CompactAbort)
        } else if self
            .history()
            .state()
            .status()
            .is_some_and(|s| s.failable())
        {
            Some(HistoryUpdate::TurnResponse(AssistantEvent::Failed {
                message: ABORTED_BY_USER.into(),
                ended_at: now,
            }))
        } else {
            None
        };
        if let Some(event) = event {
            self.handle_history(g, event, effects)?;
        }
        // pushed last: the mirror must apply the generation bump (and the
        // turn's Failed response) before AbortTasks reaps in-flight tools —
        // those finalizations emit at the new generation, and a mirror still
        // on the old one rejects them as a generation mismatch (H2)
        effects.push(Effect::AbortTasks);
        Ok(())
    }

    fn init_compact(
        &mut self,
        now: u64,
        n_drop: usize,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        if n_drop == 0 {
            return Ok(());
        }
        let g = self.history().generation();
        self.handle_history(
            g,
            HistoryUpdate::CompactStart(CompactStart::new(n_drop, now)),
            effects,
        )
    }

    fn compact_turn(
        &mut self,
        now: u64,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        // resolve the input before CompactResponse(Created) lands
        let messages = self.history().compact_turn_input()?;
        self.spawn_turn(
            now,
            ToolRegistry::empty(),
            messages,
            TurnType::Compact,
            effects,
        )
    }

    fn start_turn(
        &mut self,
        now: u64,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        // clone before the Created event appends the queued assistant message
        let messages = self.history().state().messages.clone();
        self.spawn_turn(
            now,
            self.tools.clone(),
            messages,
            TurnType::Default,
            effects,
        )
    }

    fn spawn_turn(
        &mut self,
        now: u64,
        tools: ToolRegistry,
        messages: Vec<Message>,
        turn_type: TurnType,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        // resolve the fallible lookup before any history/ledger mutation: a
        // stale assistant id must fail the submit, not wedge the agent busy
        let assistant = self.assistants.assistant(&self.state.assistant)?;
        let created = AssistantEvent::Created { created_at: now };
        let created = match turn_type {
            TurnType::Default => HistoryUpdate::TurnResponse(created),
            TurnType::Compact => HistoryUpdate::CompactResponse(created),
        };
        let generation = self.history().generation();
        let instructions = self.history().instructions().to_string();
        self.handle_history(generation, created, effects)?;
        effects.push(Effect::StartTurn {
            id: self.ledger.register(),
            generation,
            turn_type,
            assistant,
            tools,
            instructions,
            messages,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::history::TurnStatus;
    use crate::llm::history::message::OutputContent;
    use crate::llm::history::message::OutputItem;
    use crate::tools::todo::TodoArguments;
    use crate::tools::todo::TodoCall;
    use crate::tools::todo::TodoResult;

    impl AgentCore {
        /// core over the fake assistant pool ("test" + "test2")
        pub fn fake() -> Self {
            let pool = Arc::new(AssistantPool::fake().0);
            let state = AgentState::new("test".into(), String::new(), String::new());
            Self::new(state, pool)
        }
    }

    /// drive one event, returning the produced effects
    fn drive(
        core: &mut AgentCore,
        now: u64,
        event: CoreEvent,
    ) -> (Result<()>, Vec<Effect>) {
        let mut effects = Vec::new();
        let result = core.handle(now, event, &mut effects);
        (result, effects)
    }

    fn submit(
        text: &str,
        generation: HistoryGeneration,
    ) -> CoreEvent {
        CoreEvent::Submit(UserPrompt {
            text: text.into(),
            generation: Some(generation),
        })
    }

    fn message(text: &str) -> CoreEvent {
        CoreEvent::Message(UserMessage::new(text.into(), 5))
    }

    /// id of the task the effects started
    fn started_task(effects: &[Effect]) -> TaskId {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::StartTurn { id, .. } | Effect::RunTool { id, .. } => Some(*id),
                _ => None,
            })
            .expect("no task-starting effect")
    }

    fn todo_item(output: Option<std::result::Result<TodoResult, String>>) -> ToolCallItem {
        ToolCallItem {
            id: Some("call-1".into()),
            call_id: "call-1".into(),
            task: Box::new(TodoCall {
                arguments: Some(TodoArguments::default()),
                meta: None,
                output,
            }),
            token_count: 0,
            started_at: 2,
            ended_at: Some(3),
            ready_at: None,
        }
    }

    fn todo_call(output: Option<std::result::Result<TodoResult, String>>) -> HistoryUpdate {
        HistoryUpdate::TurnResponse(AssistantEvent::Item(Box::new(AssistantItem::ToolCall(
            todo_item(output),
        ))))
    }

    fn text_output(
        id: &str,
        text: &str,
    ) -> HistoryUpdate {
        HistoryUpdate::TurnResponse(AssistantEvent::Item(Box::new(AssistantItem::Output(
            OutputItem {
                id: id.into(),
                content: vec![OutputContent::Text(text.into())],
                token_count: 0,
                started_at: 1,
                ended_at: None,
            },
        ))))
    }

    macro_rules! assert_handled {
        ($core:expr, $now:expr, $event:expr, @$snapshot:literal) => {{
            let (result, effects) = drive($core, $now, $event);
            result.unwrap();
            insta::assert_yaml_snapshot!(($core.derive_status(), effects), @$snapshot);
        }};
    }

    macro_rules! assert_rejected {
        ($core:expr, $now:expr, $event:expr, @$snapshot:literal) => {{
            let (result, effects) = drive($core, $now, $event);
            insta::assert_yaml_snapshot!(
                (result.unwrap_err().to_string(), $core.derive_status(), effects),
                @$snapshot
            );
        }};
    }

    #[test]
    fn submit_starts_turn() {
        let mut core = AgentCore::fake();
        assert_handled!(&mut core, 7, submit("hi", 0), @"
        - Normal: InProgress
        - - Emit:
              HistoryUpdate:
                - 0
                - UserMessage:
                    text: hi
                    token_count: 1
                    created_at: 7
          - Save
          - Emit:
              HistoryUpdate:
                - 0
                - GenerationIncremented
          - Emit:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Created:
                      created_at: 7
          - StartTurn:
              id: 0
              generation: 1
              turn_type: Default
              assistant: test
          - Emit:
              StatusUpdate:
                Normal: InProgress
        ");
    }

    #[test]
    fn submit_with_stale_generation_is_rejected() {
        let mut core = AgentCore::fake();
        assert_rejected!(&mut core, 7, submit("hi", 1), @r#"
        - "history generation mismatch: expected 0"
        - Normal: Idle
        - []
        "#);
    }

    #[test]
    fn submit_while_busy_queues_as_pending() {
        let mut core = AgentCore::fake();
        core.ledger.register();
        assert_handled!(&mut core, 7, submit("hi", 0), @"
        - Normal: InProgress
        - - Save
          - Emit:
              StatusUpdate:
                Normal: InProgress
        ");
        assert_eq!(core.state.pending_messages.len(), 1);
    }

    /// M8: a mid-turn submit — even one stamped with a nonsense generation —
    /// is queued and flushes into its own turn at turn end, so typed input
    /// is never destroyed by a busy rejection
    #[test]
    fn busy_submit_flushes_into_a_turn_at_turn_end() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 0));
        let turn = started_task(&effects);
        drive(&mut core, 2, submit("steer", 999)).0.unwrap();
        assert_eq!(core.state.pending_messages.len(), 1);

        drive(
            &mut core,
            3,
            CoreEvent::TaskEvent(
                turn,
                1,
                HistoryUpdate::TurnResponse(AssistantEvent::Completed { ended_at: 3 }),
            ),
        )
        .0
        .unwrap();
        let (result, effects) = drive(&mut core, 4, CoreEvent::TaskDone(turn, Ok(())));
        result.unwrap();
        assert!(serde_json::to_string(&effects).unwrap().contains("steer"));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::StartTurn { .. }))
        );
        assert!(core.state.pending_messages.is_empty());
    }

    /// finding 3: a stale assistant id (e.g. removed from config after a
    /// restore) fails the submit cleanly — no queued turn message, no
    /// dangling ledger task — and the idle agent accepts the fix
    #[test]
    fn submit_with_unknown_assistant_fails_without_wedging() {
        let mut core = AgentCore::fake();
        core.state.assistant = "gone".into();
        let (result, effects) = drive(&mut core, 7, submit("hi", 0));
        similar_asserts::assert_eq!(
            result.unwrap_err().to_string(),
            "unknown assistant \"gone\"".to_string()
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::StartTurn { .. })),
            "{effects:?}"
        );
        core.idle().unwrap();
        // `Agent` applies SetAssistant; a resubmit then turns normally
        drive(&mut core, 8, CoreEvent::SetAssistant("test".into()))
            .0
            .unwrap();
        core.state.assistant = "test".into();
        drive(&mut core, 9, submit("retry", 1)).0.unwrap();
        similar_asserts::assert_eq!(
            core.derive_status(),
            ActivityStatus::Normal(TurnStatus::InProgress)
        );
    }

    #[test]
    fn abort_fails_inflight_turn() {
        let mut core = AgentCore::fake();
        drive(&mut core, 7, submit("hi", 0)).0.unwrap();
        assert_handled!(&mut core, 9, CoreEvent::Abort, @"
        - Normal:
            Failed: aborted by user
        - - Emit:
              HistoryUpdate:
                - 1
                - GenerationIncremented
          - Emit:
              HistoryUpdate:
                - 2
                - TurnResponse:
                    Failed:
                      message: aborted by user
                      ended_at: 9
          - Save
          - AbortTasks
          - Emit:
              StatusUpdate:
                Normal:
                  Failed: aborted by user
        ");
        assert!(matches!(
            core.history().state().last(),
            Some(Message::Assistant(crate::llm::history::message::AssistantMessage {
                status: crate::llm::history::message::AssistantStatus::Error(msg),
                ..
            })) if msg == ABORTED_BY_USER
        ));
    }

    #[test]
    fn retry_after_compact_failure_restarts_compaction() {
        let mut core = AgentCore::fake();
        let history = core.history_mut();
        history
            .handle(
                0,
                HistoryUpdate::UserMessage(UserMessage::new("x".repeat(2000), 0)),
            )
            .unwrap();
        history
            .handle(0, HistoryUpdate::CompactStart(CompactStart::new(1, 0)))
            .unwrap();
        history
            .handle(
                0,
                HistoryUpdate::CompactResponse(AssistantEvent::Created { created_at: 0 }),
            )
            .unwrap();
        history
            .handle(
                0,
                HistoryUpdate::CompactResponse(AssistantEvent::Failed {
                    message: "oops".into(),
                    ended_at: 1,
                }),
            )
            .unwrap();

        assert_handled!(&mut core, 7, CoreEvent::Retry, @"
        - Compact: InProgress
        - - Emit:
              HistoryUpdate:
                - 0
                - GenerationIncremented
          - Emit:
              HistoryUpdate:
                - 1
                - CompactResponse:
                    Created:
                      created_at: 7
          - Save
          - StartTurn:
              id: 0
              generation: 1
              turn_type: Compact
              assistant: test
          - Emit:
              StatusUpdate:
                Compact: InProgress
        ");
        assert!(core.history().compacting());
    }

    #[test]
    fn compact_zero_messages_is_rejected() {
        let mut core = AgentCore::fake();
        core.history_mut()
            .handle(
                0,
                HistoryUpdate::UserMessage(UserMessage::new("short".into(), 0)),
            )
            .unwrap();
        assert_rejected!(&mut core, 7, CoreEvent::Compact(0), @"
        - no compact available
        - Normal: Idle
        - []
        ");
        assert!(core.ledger.idle());
        assert!(!core.history().compacting());
    }

    #[test]
    fn compact_failure_does_not_start_normal_turn() {
        let mut core = AgentCore::fake();
        let history = core.history_mut();
        history
            .handle(
                0,
                HistoryUpdate::UserMessage(UserMessage::new("first".into(), 0)),
            )
            .unwrap();
        history
            .handle(0, HistoryUpdate::CompactStart(CompactStart::new(1, 0)))
            .unwrap();
        history
            .handle(
                0,
                HistoryUpdate::CompactResponse(AssistantEvent::Created { created_at: 0 }),
            )
            .unwrap();
        history
            .handle(
                0,
                HistoryUpdate::CompactResponse(AssistantEvent::Failed {
                    message: "oops".into(),
                    ended_at: 1,
                }),
            )
            .unwrap();
        let tid = core.ledger.register();

        assert_handled!(&mut core, 7, CoreEvent::TaskDone(tid, Ok(())), @"
        - Compact:
            Failed: oops
        - - Emit:
              StatusUpdate:
                Compact:
                  Failed: oops
        ");
        assert!(core.ledger.idle());
        assert!(core.history().compacting());
    }

    #[test]
    fn task_failure_emits_error_and_keeps_failed_status() {
        let mut core = AgentCore::fake();
        core.state.status = ActivityStatus::Normal(TurnStatus::InProgress);
        let history = core.history_mut();
        history
            .handle(
                0,
                HistoryUpdate::UserMessage(UserMessage::new("first".into(), 0)),
            )
            .unwrap();
        history
            .handle(
                0,
                HistoryUpdate::TurnResponse(AssistantEvent::Created { created_at: 0 }),
            )
            .unwrap();
        history
            .handle(
                0,
                HistoryUpdate::TurnResponse(AssistantEvent::Failed {
                    message: "oops".into(),
                    ended_at: 1,
                }),
            )
            .unwrap();
        let tid = core.ledger.register();

        assert_handled!(&mut core, 7, CoreEvent::TaskDone(tid, Err("oops".into())), @"
        - Normal:
            Failed: oops
        - - Emit:
              Error: oops
          - Emit:
              StatusUpdate:
                Normal:
                  Failed: oops
        ");
    }

    #[test]
    fn set_assistant_rejected_while_busy() {
        let mut core = AgentCore::fake();
        core.ledger.register();
        assert_rejected!(&mut core, 7, CoreEvent::SetAssistant("test2".into()), @"
        - agent is busy
        - Normal: InProgress
        - []
        ");
        assert_eq!(core.state.assistant, "test");
    }

    #[test]
    fn set_assistant_resolves_to_single_effect() {
        let mut core = AgentCore::fake();
        assert_handled!(&mut core, 7, CoreEvent::SetAssistant("test2".into()), @"
        - Normal: Idle
        - - SetAssistant: test2
        ");
        // core state untouched: `Agent` applies it after the save succeeds
        assert_eq!(core.state.assistant, "test");
    }

    #[test]
    fn set_assistant_unknown_id_is_rejected() {
        let mut core = AgentCore::fake();
        assert_rejected!(&mut core, 7, CoreEvent::SetAssistant("nope".into()), @r#"
        - "unknown assistant \"nope\""
        - Normal: Idle
        - []
        "#);
    }

    #[test]
    fn abort_while_compacting_aborts_compact() {
        let mut core = AgentCore::fake();
        core.history_mut()
            .handle(
                0,
                HistoryUpdate::UserMessage(UserMessage::new("first".into(), 0)),
            )
            .unwrap();
        drive(&mut core, 5, CoreEvent::Compact(1)).0.unwrap();

        assert_handled!(&mut core, 9, CoreEvent::Abort, @"
        - Normal: Idle
        - - Emit:
              HistoryUpdate:
                - 0
                - GenerationIncremented
          - Emit:
              HistoryUpdate:
                - 1
                - CompactAbort
          - Save
          - AbortTasks
          - Emit:
              StatusUpdate:
                Normal: Idle
        ");
        assert!(!core.history().compacting());
    }

    #[test]
    fn abort_while_idle_emits_no_history_event() {
        let mut core = AgentCore::fake();
        assert_handled!(&mut core, 9, CoreEvent::Abort, @"
        - Normal: Idle
        - - Emit:
              HistoryUpdate:
                - 0
                - GenerationIncremented
          - AbortTasks
        ");
    }

    #[test]
    fn task_failure_after_abort_surfaces_error_without_reply() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 0));
        let tid = started_task(&effects);
        drive(&mut core, 2, CoreEvent::Abort).0.unwrap();

        assert_handled!(&mut core, 3, CoreEvent::TaskDone(tid, Err("stream closed".into())), @"
        - Normal:
            Failed: aborted by user
        - - Emit:
              Error: stream closed
        ");
    }

    #[test]
    fn task_event_after_abort_is_dropped() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 0));
        let tid = started_task(&effects);
        drive(&mut core, 2, CoreEvent::Abort).0.unwrap();

        assert_handled!(&mut core, 3, CoreEvent::TaskEvent(tid, 1, text_output("out", "late")), @"
        - Normal:
            Failed: aborted by user
        - []
        ");
    }

    #[test]
    fn task_done_starts_followup_turn_after_tool_resolves() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 0));
        let turn = started_task(&effects);

        let (result, effects) = drive(&mut core, 2, CoreEvent::TaskEvent(turn, 1, todo_call(None)));
        result.unwrap();
        let tool = started_task(&effects);
        insta::assert_yaml_snapshot!(effects, @r#"
        - RunTool:
            id: 1
            call:
              id: call-1
              call_id: call-1
              name: todo
              arguments:
                current: ""
                entries: []
              meta: ~
              output: ~
              token_count: 0
              started_at: 2
              ended_at: 3
              ready_at: ~
        - Emit:
            HistoryUpdate:
              - 1
              - TurnResponse:
                  Item:
                    ToolCall:
                      id: call-1
                      call_id: call-1
                      name: todo
                      arguments:
                        current: ""
                        entries: []
                      meta: ~
                      output: ~
                      token_count: 0
                      started_at: 2
                      ended_at: 3
                      ready_at: ~
        - Save
        "#);

        drive(
            &mut core,
            3,
            CoreEvent::TaskEvent(
                turn,
                1,
                HistoryUpdate::TurnResponse(AssistantEvent::Completed { ended_at: 3 }),
            ),
        )
        .0
        .unwrap();
        // the turn task finishing leaves the tool task pending: no new turn yet
        let (result, effects) = drive(&mut core, 4, CoreEvent::TaskDone(turn, Ok(())));
        result.unwrap();
        assert!(effects.is_empty());

        // the reaper's terminal resolves the slot and starts the follow-up
        assert_handled!(
            &mut core, 6,
            CoreEvent::ToolResolved(tool, Box::new(todo_item(Some(Ok(TodoResult {}))))),
            @r#"
        - Normal: InProgress
        - - Emit:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Item:
                      ToolCall:
                        id: call-1
                        call_id: call-1
                        name: todo
                        arguments:
                          current: ""
                          entries: []
                        meta: ~
                        output:
                          Ok: {}
                        token_count: 0
                        started_at: 2
                        ended_at: 3
                        ready_at: ~
          - Save
          - Emit:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Created:
                      created_at: 6
          - StartTurn:
              id: 2
              generation: 1
              turn_type: Default
              assistant: test
        "#);
    }

    #[test]
    fn reaped_tool_failure_patches_slot_and_starts_followup() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 0));
        let turn = started_task(&effects);
        let (_, effects) = drive(&mut core, 2, CoreEvent::TaskEvent(turn, 1, todo_call(None)));
        let tool = started_task(&effects);
        drive(
            &mut core,
            3,
            CoreEvent::TaskEvent(
                turn,
                1,
                HistoryUpdate::TurnResponse(AssistantEvent::Completed { ended_at: 3 }),
            ),
        )
        .0
        .unwrap();
        drive(&mut core, 4, CoreEvent::TaskDone(turn, Ok(())))
            .0
            .unwrap();

        // panic/cancel finalize: slot gets the error, ledger unsticks, and
        // the model sees the failure next turn
        assert_handled!(
            &mut core, 5,
            CoreEvent::ToolFailed {
                id: tool,
                call_id: "call-1".into(),
                error: "tool panicked: boom; partial output:\nabc".into(),
            },
            @r#"
        - Normal: InProgress
        - - Emit:
              HistoryUpdate:
                - 1
                - ToolCallFailed:
                    call_id: call-1
                    error: "tool panicked: boom; partial output:\nabc"
          - Save
          - Emit:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Created:
                      created_at: 5
          - StartTurn:
              id: 2
              generation: 1
              turn_type: Default
              assistant: test
        "#);
        // the slot itself holds the error (the follow-up Created appended a
        // fresh assistant message after it)
        assert!(
            core.history()
                .state()
                .iter()
                .filter_map(|m| m.try_as_assistant_ref())
                .flat_map(|m| m.content.values())
                .any(|item| matches!(
                    item,
                    AssistantItem::ToolCall(call)
                        if call.task.output().is_some_and(|o| o.contains("boom"))
                ))
        );
    }

    #[test]
    fn message_while_idle_appends_and_wakes() {
        let mut core = AgentCore::fake();
        assert_handled!(&mut core, 7, message("[from: kid]\nhi"), @r#"
        - Normal: InProgress
        - - Emit:
              HistoryUpdate:
                - 0
                - UserMessage:
                    text: "[from: kid]\nhi"
                    token_count: 5
                    created_at: 5
          - Save
          - Emit:
              HistoryUpdate:
                - 0
                - TurnResponse:
                    Created:
                      created_at: 7
          - StartTurn:
              id: 0
              generation: 0
              turn_type: Default
              assistant: test
          - Emit:
              StatusUpdate:
                Normal: InProgress
        "#);
        assert!(core.state.pending_messages.is_empty());
    }

    #[test]
    fn message_while_busy_buffers_then_flushes_on_idle() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 0));
        let turn = started_task(&effects);

        // turn still streaming: buffered, save only
        assert_handled!(&mut core, 2, message("[from: kid]\nearly bird"), @"
        - Normal: InProgress
        - - Save
        ");
        assert_eq!(core.state.pending_messages.len(), 1);

        drive(
            &mut core,
            3,
            CoreEvent::TaskEvent(
                turn,
                1,
                HistoryUpdate::TurnResponse(AssistantEvent::Completed { ended_at: 3 }),
            ),
        )
        .0
        .unwrap();
        // idle: the buffer flushes and wakes the agent
        assert_handled!(&mut core, 4, CoreEvent::TaskDone(turn, Ok(())), @r#"
        - Normal: InProgress
        - - Emit:
              HistoryUpdate:
                - 1
                - UserMessage:
                    text: "[from: kid]\nearly bird"
                    token_count: 6
                    created_at: 5
          - Save
          - Emit:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Created:
                      created_at: 4
          - StartTurn:
              id: 1
              generation: 1
              turn_type: Default
              assistant: test
        "#);
        assert!(core.state.pending_messages.is_empty());
    }

    #[test]
    fn message_while_compacting_stays_buffered() {
        let mut core = AgentCore::fake();
        core.history_mut()
            .handle(
                0,
                HistoryUpdate::UserMessage(UserMessage::new("first".into(), 0)),
            )
            .unwrap();
        let (_, effects) = drive(&mut core, 5, CoreEvent::Compact(1));
        let compact = started_task(&effects);

        assert_handled!(&mut core, 6, message("[from: kid]\nmid compact"), @"
        - Compact: InProgress
        - - Save
        ");
        assert_eq!(core.state.pending_messages.len(), 1);

        // a failed compact leaves the history compacting: the buffer holds
        drive(
            &mut core,
            7,
            CoreEvent::TaskEvent(
                compact,
                0,
                HistoryUpdate::CompactResponse(AssistantEvent::Failed {
                    message: "oops".into(),
                    ended_at: 7,
                }),
            ),
        )
        .0
        .unwrap();
        let (result, effects) = drive(&mut core, 8, CoreEvent::TaskDone(compact, Ok(())));
        result.unwrap();
        assert!(
            !serde_json::to_string(&effects)
                .unwrap()
                .contains("mid compact")
        );
        assert_eq!(core.state.pending_messages.len(), 1);
        assert!(core.history().compacting());
    }

    #[test]
    fn stale_submit_is_rejected_before_flush() {
        let mut core = AgentCore::fake();
        drive(&mut core, 1, submit("hi", 0)).0.unwrap();
        drive(&mut core, 2, message("[from: kid]\nstranded"))
            .0
            .unwrap();
        // abort clears the ledger but deliberately never flushes
        drive(&mut core, 3, CoreEvent::Abort).0.unwrap();
        assert_eq!(core.state.pending_messages.len(), 1);

        // stale generation: rejected before the flush — the buffer is intact
        let (result, effects) = drive(&mut core, 4, submit("late", 1));
        assert!(result.is_err());
        assert!(effects.is_empty());
        assert_eq!(core.state.pending_messages.len(), 1);
    }

    #[test]
    fn submit_flushes_stranded_buffer_before_user_message() {
        let mut core = AgentCore::fake();
        drive(&mut core, 1, submit("hi", 0)).0.unwrap();
        drive(&mut core, 2, message("[from: kid]\nstranded"))
            .0
            .unwrap();
        drive(&mut core, 3, CoreEvent::Abort).0.unwrap();

        let (result, effects) = drive(&mut core, 4, submit("continue", 2));
        result.unwrap();
        let positions: Vec<usize> = ["stranded", "continue"]
            .iter()
            .map(|needle| {
                effects
                    .iter()
                    .position(|e| serde_json::to_string(e).unwrap().contains(*needle))
                    .unwrap_or_else(|| panic!("{needle} not delivered"))
            })
            .collect();
        assert!(
            positions[0] < positions[1],
            "pending message must precede the user message"
        );
        assert!(core.state.pending_messages.is_empty());
    }

    #[test]
    fn resume_flushes_buffered_messages_and_starts_turn() {
        let mut core = AgentCore::fake();
        drive(&mut core, 1, submit("hi", 0)).0.unwrap();
        drive(&mut core, 2, message("[from: kid]\npersisted"))
            .0
            .unwrap();
        drive(&mut core, 3, CoreEvent::Abort).0.unwrap();

        // simulated restart: rebuild the core from the persisted state
        let json = serde_json::to_value(&core.state).unwrap();
        let restored: AgentState = serde_json::from_value(json).unwrap();
        let mut restored = AgentCore::new(restored, core.assistants.clone());
        assert_eq!(restored.state.pending_messages.len(), 1);

        // resume (the startup wake) delivers the buffered message and starts
        // its turn — no submit or other poke needed (M3)
        let mut effects = Vec::new();
        restored.resume(5, &mut effects).unwrap();
        assert!(restored.state.pending_messages.is_empty());
        assert!(
            serde_json::to_string(&effects)
                .unwrap()
                .contains("persisted")
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::StartTurn { .. }))
        );
    }

    #[test]
    fn buffered_message_survives_restore_and_flushes_on_submit() {
        let mut core = AgentCore::fake();
        drive(&mut core, 1, submit("hi", 0)).0.unwrap();
        drive(&mut core, 2, message("[from: kid]\npersisted"))
            .0
            .unwrap();
        drive(&mut core, 3, CoreEvent::Abort).0.unwrap();

        let json = serde_json::to_value(&core.state).unwrap();
        let restored: AgentState = serde_json::from_value(json).unwrap();
        let mut restored = AgentCore::new(restored, core.assistants.clone());
        assert_eq!(restored.state.pending_messages.len(), 1);

        // restored generation resets to 0: it is #[serde(skip)]
        let (result, effects) = drive(&mut restored, 5, submit("go", 0));
        result.unwrap();
        assert!(
            serde_json::to_string(&effects)
                .unwrap()
                .contains("persisted")
        );
        assert!(restored.state.pending_messages.is_empty());
    }

    #[test]
    fn spawn_call_captures_history_at_dispatch() {
        use crate::tools::agent::spawn::SpawnArguments;
        use crate::tools::agent::spawn::SpawnCall;

        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 0));
        let turn = started_task(&effects);

        let capture_of = |core: &mut AgentCore, item: ToolCallItem| {
            let update = HistoryUpdate::TurnResponse(AssistantEvent::Item(Box::new(
                AssistantItem::ToolCall(item),
            )));
            let (result, effects) = drive(core, 2, CoreEvent::TaskEvent(turn, 1, update));
            result.unwrap();
            effects
                .into_iter()
                .find_map(|e| match e {
                    Effect::RunTool { capture, .. } => Some(capture),
                    _ => None,
                })
                .expect("no RunTool effect")
        };

        let spawn_item = |call_id: &str, inherit| ToolCallItem {
            id: Some(call_id.into()),
            call_id: call_id.into(),
            task: Box::new(SpawnCall {
                arguments: Some(SpawnArguments {
                    prompt: "go".into(),
                    inherit_context: inherit,
                }),
                meta: None,
                output: None,
            }),
            token_count: 0,
            started_at: 2,
            ended_at: Some(3),
            ready_at: None,
        };

        // inherit=true: the parent's conversation, snapshotted at dispatch
        let capture = capture_of(&mut core, spawn_item("call-1", true)).unwrap();
        assert!(
            capture
                .state()
                .messages
                .iter()
                .any(|m| matches!(m, Message::User(u) if u.text == "hi"))
        );
        assert_eq!(capture.generation(), 0);
        // inherit=false: an empty history
        let capture = capture_of(&mut core, spawn_item("call-2", false)).unwrap();
        assert!(capture.state().messages.is_empty());
        // non-spawn tools carry no capture
        assert!(capture_of(&mut core, todo_item(None)).is_none());
    }

    #[test]
    fn task_done_returns_to_idle() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 0));
        let tid = started_task(&effects);
        drive(
            &mut core,
            2,
            CoreEvent::TaskEvent(tid, 1, text_output("out", "done")),
        )
        .0
        .unwrap();
        drive(
            &mut core,
            3,
            CoreEvent::TaskEvent(
                tid,
                1,
                HistoryUpdate::TurnResponse(AssistantEvent::Completed { ended_at: 3 }),
            ),
        )
        .0
        .unwrap();

        assert_handled!(&mut core, 4, CoreEvent::TaskDone(tid, Ok(())), @"
        - Normal: Idle
        - - Emit:
              StatusUpdate:
                Normal: Idle
        ");
    }

    #[test]
    fn task_done_failure_surfaces_error_and_failed_status() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 0));
        let tid = started_task(&effects);
        drive(
            &mut core,
            2,
            CoreEvent::TaskEvent(
                tid,
                1,
                HistoryUpdate::TurnResponse(AssistantEvent::Failed {
                    message: "boom".into(),
                    ended_at: 2,
                }),
            ),
        )
        .0
        .unwrap();

        assert_handled!(&mut core, 3, CoreEvent::TaskDone(tid, Err("boom".into())), @"
        - Normal:
            Failed: boom
        - - Emit:
              Error: boom
          - Emit:
              StatusUpdate:
                Normal:
                  Failed: boom
        ");
    }

    #[test]
    fn resolved_tool_call_is_not_rerun() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 0));
        let tid = started_task(&effects);

        assert_handled!(
            &mut core, 2,
            CoreEvent::TaskEvent(tid, 1, todo_call(Some(Ok(TodoResult {})))),
            @r#"
        - Normal: InProgress
        - - Emit:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Item:
                      ToolCall:
                        id: call-1
                        call_id: call-1
                        name: todo
                        arguments:
                          current: ""
                          entries: []
                        meta: ~
                        output:
                          Ok: {}
                        token_count: 0
                        started_at: 2
                        ended_at: 3
                        ready_at: ~
          - Save
        "#
        );
    }

    #[test]
    fn retry_failed_turn_starts_normal_turn() {
        let mut core = AgentCore::fake();
        let history = core.history_mut();
        history
            .handle(
                0,
                HistoryUpdate::UserMessage(UserMessage::new("hi".into(), 0)),
            )
            .unwrap();
        history
            .handle(
                0,
                HistoryUpdate::TurnResponse(AssistantEvent::Created { created_at: 0 }),
            )
            .unwrap();
        history
            .handle(
                0,
                HistoryUpdate::TurnResponse(AssistantEvent::Failed {
                    message: "oops".into(),
                    ended_at: 1,
                }),
            )
            .unwrap();

        assert_handled!(&mut core, 7, CoreEvent::Retry, @"
        - Normal: InProgress
        - - Emit:
              HistoryUpdate:
                - 0
                - GenerationIncremented
          - Emit:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Created:
                      created_at: 7
          - Save
          - StartTurn:
              id: 0
              generation: 1
              turn_type: Default
              assistant: test
          - Emit:
              StatusUpdate:
                Normal: InProgress
        ");
    }

    #[test]
    fn undo_pops_messages() {
        let mut core = AgentCore::fake();
        let history = core.history_mut();
        for text in ["first", "second"] {
            history
                .handle(
                    0,
                    HistoryUpdate::UserMessage(UserMessage::new(text.into(), 0)),
                )
                .unwrap();
        }

        assert_handled!(&mut core, 7, CoreEvent::Undo(1), @"
        - Normal: Idle
        - - Emit:
              HistoryUpdate:
                - 0
                - GenerationIncremented
          - Emit:
              HistoryUpdate:
                - 1
                - Pop: 1
          - Save
        ");
        assert_eq!(core.history().state().messages.len(), 1);
    }
}

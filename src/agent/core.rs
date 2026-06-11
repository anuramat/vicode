//! pure agent decision logic: no IO, no clocks, no channels, no tokio. The
//! shell (`handle.rs`) translates wire events into [`CoreEvent`]s, calls
//! [`AgentCore::handle`], and interprets the produced [`Effect`]s in order.

use std::sync::Arc;

use anyhow::Result;

use crate::agent::AgentState;
use crate::agent::AgentStatus;
use crate::agent::handle::ParentEvent;
use crate::agent::handle::TurnResult;
use crate::agent::handle::UserPrompt;
use crate::agent::id::AgentId;
use crate::agent::router::SubagentSpawnSnapshot;
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
use crate::llm::history::TurnStatus;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::Message;
use crate::llm::history::message::ToolCallItem;
use crate::llm::history::message::UserMessage;
use crate::llm::provider::assistant::Assistant;
use crate::llm::provider::assistant::AssistantPool;

pub const ABORTED_BY_USER: &str = "aborted by user";

#[derive(Debug)]
pub struct AgentCore {
    pub state: AgentState,
    pub ledger: TaskLedger,
    pub tools: ToolRegistry,
    pub assistants: Arc<AssistantPool>,
    /// mirrors the shell's pending-done oneshot slot
    pub pending_done: bool,
}

/// `AgentEvent` minus IO payloads: oneshots are stripped by the shell
#[derive(Debug)]
pub enum CoreEvent {
    TaskDone(TaskId, Result<(), String>),
    TaskEvent(TaskId, HistoryGeneration, HistoryUpdate),
    /// the flag tracks whether the shell staged a done oneshot
    Submit(UserPrompt, bool),
    Compact(usize),
    Retry,
    Abort,
    Undo(usize),
    SetAssistant(String),
    Duplicate(AgentId),
    Snapshot,
}

/// fat effects: all decision-time state is captured at push time, so the
/// shell interprets them blindly and strictly in order
#[derive(Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum Effect {
    Emit(ParentEvent),
    /// marker; the shell serializes the core state as of this position
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
        generation: HistoryGeneration,
        call: ToolCallItem,
    },
    StartReplicas {
        id: TaskId,
        generation: HistoryGeneration,
        n: usize,
    },
    AbortTasks,
    /// shell: take the done slot and send (no-op if empty)
    ReplyDone(TurnResult),
    /// shell: move this event's staged done oneshot into the slot
    StoreDone,
    ReplySnapshot(SubagentSpawnSnapshot),
    /// shell: save state-with-new; on success apply + emit `AssistantSet`
    SetAssistant(Assistant),
    Duplicate(AgentId),
}

impl AgentCore {
    forward! {
        history: History = self.state.context.history;
    }

    pub fn new(
        state: AgentState,
        assistants: Arc<AssistantPool>,
    ) -> Self {
        let tools = tools_for_depth(state.max_depth);
        Self {
            state,
            ledger: TaskLedger::default(),
            tools,
            assistants,
            pending_done: false,
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
            Submit(prompt, has_done) => self.submit(now, prompt, has_done, effects),
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
                effects.push(Effect::SetAssistant(new));
                Ok(())
            }
            Duplicate(aid) => {
                self.idle()?;
                effects.push(Effect::Duplicate(aid));
                Ok(())
            }
            // no idle gate: the subagent tool snapshots a busy parent
            Snapshot => {
                effects.push(Effect::ReplySnapshot(SubagentSpawnSnapshot {
                    commit: self.state.context.commit.clone(),
                    assistant_id: self.state.assistant.id.clone(),
                    history: self.state.context.history.clone(),
                    max_depth: self.state.max_depth,
                }));
                Ok(())
            }
        };
        if result.is_ok() {
            self.sync_status(effects);
        }
        result
    }

    pub fn derive_status(&self) -> AgentStatus {
        let busy = !self.ledger.idle();
        match self.history().activity() {
            Activity::Normal { state } => AgentStatus::Normal(state.turn_status(busy)),
            Activity::Compacting { compact, .. } => {
                AgentStatus::Compact(compact.state.turn_status(busy))
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
        self.history_mut().handle(generation, event.clone())?;
        effects.push(Effect::Emit(ParentEvent::HistoryUpdate(
            generation,
            event.clone(),
        )));
        match event {
            HistoryUpdate::TurnResponse(AssistantEvent::Item(ref item)) => {
                self.run_tool_call(item, effects);
            }
            HistoryUpdate::TurnResponse(AssistantEvent::Failed { message, .. })
            | HistoryUpdate::CompactResponse(AssistantEvent::Failed { message, .. }) => {
                tracing::error!("response error: {message}");
            }
            HistoryUpdate::GenerationIncremented
            | HistoryUpdate::TurnResponse(AssistantEvent::Delta(_))
            | HistoryUpdate::CompactResponse(AssistantEvent::Delta(_)) => return Ok(()),
            _ => {}
        }
        // TODO save less often; save on errors
        effects.push(Effect::Save);
        Ok(())
    }

    fn run_tool_call(
        &mut self,
        item: &AssistantItem,
        effects: &mut Vec<Effect>,
    ) {
        let AssistantItem::ToolCall(call) = item else {
            return;
        };
        if call.task.output().is_some() {
            return;
        }
        effects.push(Effect::RunTool {
            id: self.ledger.register(),
            generation: self.history().generation(),
            call: call.clone(),
        });
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
        if !self.ledger.finish(&id) {
            return Ok(());
        }
        if self.ledger.idle() {
            if self.history().state().needs_another_turn() && !self.history().compacting() {
                self.start_turn(now, effects)?;
            } else {
                self.fire_pending_done(effects);
            }
        }
        Ok(())
    }

    fn fire_pending_done(
        &mut self,
        effects: &mut Vec<Effect>,
    ) {
        if !self.pending_done {
            return;
        }
        self.pending_done = false;
        let result = match self.derive_status() {
            AgentStatus::Normal(TurnStatus::Failed(msg))
            | AgentStatus::Compact(TurnStatus::Failed(msg)) => TurnResult::Failed(msg),
            _ => TurnResult::Success {
                last_text: self.history().state().last_text_output().ok(),
            },
        };
        effects.push(Effect::ReplyDone(result));
    }

    fn submit(
        &mut self,
        now: u64,
        prompt: UserPrompt,
        has_done: bool,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        // on Err the shell drops the staged oneshot: receiver sees RecvError
        self.idle()?;
        if self.pending_done {
            effects.push(Effect::ReplyDone(TurnResult::Failed(
                ABORTED_BY_USER.into(),
            )));
        }
        effects.push(Effect::StoreDone);
        self.pending_done = has_done;
        if let Err(e) = self.submit_inner(now, prompt, effects) {
            self.pending_done = false;
            effects.push(Effect::ReplyDone(TurnResult::Failed(e.to_string())));
            return Err(e);
        }
        Ok(())
    }

    fn submit_inner(
        &mut self,
        now: u64,
        UserPrompt {
            text,
            multiplier,
            generation,
        }: UserPrompt,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        self.handle_history(
            generation,
            HistoryUpdate::UserMessage(UserMessage::new(text, now)),
            effects,
        )?;
        self.increment_generation(effects)?;
        if multiplier <= 1 {
            self.start_turn(now, effects)?;
        } else {
            effects.push(Effect::StartReplicas {
                id: self.ledger.register(),
                generation: self.history().generation(),
                n: multiplier,
            });
        }
        Ok(())
    }

    fn abort(
        &mut self,
        now: u64,
        effects: &mut Vec<Effect>,
    ) -> Result<()> {
        let had_done = std::mem::take(&mut self.pending_done);
        self.ledger.clear();
        effects.push(Effect::AbortTasks);
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
        // positional saves land before ReplyDone: the done receiver may
        // delete and thereby abort this very loop
        if had_done {
            effects.push(Effect::ReplyDone(TurnResult::Failed(
                ABORTED_BY_USER.into(),
            )));
        }
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
            assistant: self.state.assistant.clone(),
            tools,
            instructions,
            messages,
        });
        Ok(())
    }
}

/// Tool set for an agent with `max_depth` remaining subagent budget.
fn tools_for_depth(max_depth: u32) -> ToolRegistry {
    if max_depth > 0 {
        return TOOL_REGISTRY.clone();
    }
    TOOL_REGISTRY.without([crate::tools::subagent::TOOL_NAME])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::history::message::OutputContent;
    use crate::llm::history::message::OutputItem;
    use crate::tools::todo::TodoArguments;
    use crate::tools::todo::TodoCall;
    use crate::tools::todo::TodoResult;

    impl AgentCore {
        /// core over the fake assistant pool ("test" + "test2"), max_depth 1
        pub fn fake() -> Self {
            let pool = Arc::new(AssistantPool::fake().0);
            let state = AgentState::new(
                pool.assistant("test").unwrap(),
                String::new(),
                String::new(),
                1,
            );
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
        multiplier: usize,
        generation: HistoryGeneration,
    ) -> CoreEvent {
        CoreEvent::Submit(
            UserPrompt {
                text: text.into(),
                multiplier,
                generation,
            },
            true,
        )
    }

    /// id of the task the effects started
    fn started_task(effects: &[Effect]) -> TaskId {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::StartTurn { id, .. }
                | Effect::RunTool { id, .. }
                | Effect::StartReplicas { id, .. } => Some(*id),
                _ => None,
            })
            .expect("no task-starting effect")
    }

    fn todo_call(output: Option<std::result::Result<TodoResult, String>>) -> HistoryUpdate {
        HistoryUpdate::TurnResponse(AssistantEvent::Item(Box::new(AssistantItem::ToolCall(
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
            },
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
        assert_handled!(&mut core, 7, submit("hi", 1, 0), @"
        - Normal: InProgress
        - - StoreDone
          - Emit:
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
    fn submit_with_stale_generation_fails_pending_done() {
        let mut core = AgentCore::fake();
        assert_rejected!(&mut core, 7, submit("hi", 1, 1), @r#"
        - "history generation mismatch: expected 0"
        - Normal: Idle
        - - StoreDone
          - ReplyDone:
              Failed: "history generation mismatch: expected 0"
        "#);
        assert!(!core.pending_done);
    }

    #[test]
    fn submit_while_busy_keeps_pending_done() {
        let mut core = AgentCore::fake();
        core.ledger.register();
        core.pending_done = true;
        assert_rejected!(&mut core, 7, submit("hi", 1, 0), @"
        - agent is busy
        - Normal: InProgress
        - []
        ");
        assert!(core.pending_done);
    }

    #[test]
    fn abort_fails_inflight_turn_and_fires_done() {
        let mut core = AgentCore::fake();
        drive(&mut core, 7, submit("hi", 1, 0)).0.unwrap();
        assert_handled!(&mut core, 9, CoreEvent::Abort, @"
        - Normal:
            Failed: aborted by user
        - - AbortTasks
          - Emit:
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
          - ReplyDone:
              Failed: aborted by user
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
        core.state.status = AgentStatus::Normal(TurnStatus::InProgress);
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
        assert_eq!(core.state.assistant.id, "test");
    }

    #[test]
    fn set_assistant_resolves_to_single_effect() {
        let mut core = AgentCore::fake();
        assert_handled!(&mut core, 7, CoreEvent::SetAssistant("test2".into()), @"
        - Normal: Idle
        - - SetAssistant: test2
        ");
        // core state untouched: the shell applies it after the save succeeds
        assert_eq!(core.state.assistant.id, "test");
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
    fn submit_replaces_pending_done() {
        let mut core = AgentCore::fake();
        core.pending_done = true;
        assert_handled!(&mut core, 7, submit("hi", 1, 0), @"
        - Normal: InProgress
        - - ReplyDone:
              Failed: aborted by user
          - StoreDone
          - Emit:
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
        assert!(core.pending_done);
    }

    #[test]
    fn submit_multiplier_starts_replicas() {
        let mut core = AgentCore::fake();
        assert_handled!(&mut core, 7, submit("hi", 3, 0), @"
        - Normal: InProgress
        - - StoreDone
          - Emit:
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
          - StartReplicas:
              id: 0
              generation: 1
              n: 3
          - Emit:
              StatusUpdate:
                Normal: InProgress
        ");
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
        - - AbortTasks
          - Emit:
              HistoryUpdate:
                - 0
                - GenerationIncremented
          - Emit:
              HistoryUpdate:
                - 1
                - CompactAbort
          - Save
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
        - - AbortTasks
          - Emit:
              HistoryUpdate:
                - 0
                - GenerationIncremented
        ");
    }

    #[test]
    fn task_failure_after_abort_surfaces_error_without_reply() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 1, 0));
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
        let (_, effects) = drive(&mut core, 1, submit("hi", 1, 0));
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
        let (_, effects) = drive(&mut core, 1, submit("hi", 1, 0));
        let turn = started_task(&effects);

        let (result, effects) = drive(&mut core, 2, CoreEvent::TaskEvent(turn, 1, todo_call(None)));
        result.unwrap();
        let tool = started_task(&effects);
        insta::assert_yaml_snapshot!(effects, @r#"
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
        - RunTool:
            id: 1
            generation: 1
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

        drive(
            &mut core,
            5,
            CoreEvent::TaskEvent(tool, 1, todo_call(Some(Ok(TodoResult {})))),
        )
        .0
        .unwrap();
        assert_handled!(&mut core, 6, CoreEvent::TaskDone(tool, Ok(())), @"
        - Normal: InProgress
        - - Emit:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Created:
                      created_at: 6
          - Save
          - StartTurn:
              id: 2
              generation: 1
              turn_type: Default
              assistant: test
        ");
    }

    #[test]
    fn task_done_fires_done_with_last_text() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 1, 0));
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
        - - ReplyDone:
              Success:
                last_text: done
          - Emit:
              StatusUpdate:
                Normal: Idle
        ");
        assert!(!core.pending_done);
    }

    #[test]
    fn task_done_fires_done_with_derived_failure() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 1, 0));
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
          - ReplyDone:
              Failed: boom
          - Emit:
              StatusUpdate:
                Normal:
                  Failed: boom
        ");
    }

    #[test]
    fn resolved_tool_call_is_not_rerun() {
        let mut core = AgentCore::fake();
        let (_, effects) = drive(&mut core, 1, submit("hi", 1, 0));
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

    #[test]
    fn snapshot_serves_busy_parent() {
        let mut core = AgentCore::fake();
        core.ledger.register();
        assert_handled!(&mut core, 7, CoreEvent::Snapshot, @r#"
        - Normal: InProgress
        - - ReplySnapshot:
              commit: ""
              assistant_id: test
              history:
                instructions:
                  text: ""
                  token_count: 0
                activity:
                  Normal:
                    state:
                      messages: []
                      token_count: 0
                archive: []
              max_depth: 1
          - Emit:
              StatusUpdate:
                Normal: InProgress
        "#);
    }
}

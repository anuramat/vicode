//! pure agent decision logic: no IO, no clocks, no channels, no tokio. The
//! `Agent` (`shell.rs`) hands every [`AgentEvent`] to
//! [`AgentCore::handle`], and interprets the produced [`Effect`]s in order.

use std::sync::Arc;

use anyhow::Result;

use crate::agent::ActivityStatus;
use crate::agent::AgentState;
use crate::agent::event::AgentEvent;
use crate::agent::event::TaskOutput;
use crate::agent::event::TaskResult;
use crate::agent::event::UiEvent;
use crate::agent::event::UserCommand;
use crate::agent::event::UserPrompt;
use crate::agent::id::AgentId;
use crate::agent::task::ledger::Task;
use crate::agent::task::ledger::TaskId;
use crate::agent::task::ledger::TaskLedger;
use crate::agent::tool::registry::TOOL_REGISTRY;
use crate::agent::tool::registry::ToolRegistry;
use crate::config::CompactConfig;
use crate::forward;
use crate::llm::history::AssistantEvent;
use crate::llm::history::Compaction;
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
    pub compact: CompactConfig,
    /// a summary that landed mid-turn, applied at the turn boundary
    pub compaction: Option<Compaction>,
    /// a turn is due: a message arrived, or the last turn asked for a
    /// follow-up; outlives `handle` only while a turn is in flight, or while
    /// held back by the hard limit, waiting for the summary
    pub wants_turn: bool,
    /// pushed by the step in progress, handed over by `handle` and `resume`
    /// even when the step fails: changes already made must still reach the
    /// `Agent`
    effects: Vec<Effect>,
}

/// fat effects: all decision-time state is captured at push time, so the
/// `Agent` interprets them blindly and strictly in order
#[derive(Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum Effect {
    Ui(UiEvent),
    /// the derived status changed: `Agent` reports it to the router and the UI
    Status(ActivityStatus),
    /// marker; `Agent` serializes the core state as of this position
    Save,
    StartTurn {
        id: TaskId,
        assistant: Assistant,
        #[cfg_attr(test, serde(skip))]
        tools: ToolRegistry,
        #[cfg_attr(test, serde(skip))]
        instructions: String,
        #[cfg_attr(test, serde(skip))]
        messages: Vec<Message>,
    },
    /// a tool-less request whose text output is the summary
    Summarize {
        id: TaskId,
        assistant: Assistant,
        #[cfg_attr(test, serde(skip))]
        instructions: String,
        messages: Vec<Message>,
    },
    RunTool {
        id: TaskId,
        call: ToolCallItem,
        /// `spawn` with inherited context only: the parent's live history,
        /// snapshotted at dispatch via the `inherit_history` hook
        #[cfg_attr(test, serde(skip))]
        inherited_history: Option<History>,
    },
    /// cancel every task future; the core already resolved them
    AbortTasks,
    /// `Agent`: save state-with-new; on success apply + emit `AssistantSet`
    SetAssistant(String),
    /// `Agent`: clone into a new root; a failure comes back to the UI as
    /// `DuplicateFailed`
    Duplicate(AgentId),
}

impl AgentCore {
    forward! {
        history: History = self.state.context.history;
    }

    pub fn new(
        mut state: AgentState,
        assistants: Arc<AssistantPool>,
        compact: CompactConfig,
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
            compact,
            compaction: None,
            wants_turn: false,
            effects: Vec::new(),
        }
    }

    pub fn handle(
        &mut self,
        now: u64,
        event: AgentEvent,
    ) -> (Result<()>, Vec<Effect>) {
        let result = match event {
            AgentEvent::User(command) => self.command(now, command),
            AgentEvent::Message(msg) => self.deliver(now, msg),
            AgentEvent::Stream(tid, event) => match self.ledger.get(tid) {
                Some(&Task::Turn { generation }) => {
                    self.handle_history(generation, HistoryUpdate::TurnResponse(event))
                }
                _ => Ok(()),
            },
            AgentEvent::Output(tid, chunk) => {
                self.output(tid, chunk);
                Ok(())
            }
            AgentEvent::Done(tid, result) => self.task_done(now, tid, result),
        };
        self.drain(result)
    }

    /// end of a step: sync the status if it succeeded, hand over the effects
    fn drain(
        &mut self,
        result: Result<()>,
    ) -> (Result<()>, Vec<Effect>) {
        if result.is_ok() {
            self.sync_status();
        }
        (result, std::mem::take(&mut self.effects))
    }

    fn command(
        &mut self,
        now: u64,
        command: UserCommand,
    ) -> Result<()> {
        match command {
            UserCommand::Submit(prompt) => self.submit(now, prompt),
            UserCommand::Compact(n) => self.compact(now, n),
            UserCommand::Retry => {
                self.idle()?;
                self.increment_generation()?;
                self.wants_turn = true;
                self.advance(now)
            }
            UserCommand::Abort => self.abort(now),
            UserCommand::Undo(n) => {
                self.idle()?;
                let g = self.increment_generation()?;
                self.handle_history(g, HistoryUpdate::Pop(n))
            }
            UserCommand::SetAssistant(id) => {
                self.idle()?;
                let new = self.assistants.assistant(&id)?;
                self.effects.push(Effect::SetAssistant(new.id));
                Ok(())
            }
            // the rejection names the copy, so the app can drop its preview
            UserCommand::Duplicate(copy) => {
                self.effects.push(match self.idle() {
                    Ok(()) => Effect::Duplicate(copy),
                    Err(e) => Effect::Ui(UiEvent::DuplicateFailed {
                        copy,
                        error: e.to_string(),
                    }),
                });
                Ok(())
            }
        }
    }

    pub fn derive_status(&self) -> ActivityStatus {
        let busy = self.ledger.in_turn() || self.wants_turn;
        ActivityStatus {
            turn: self.history().state().turn_status(busy),
            compacting: self.compacting(),
        }
    }

    fn sync_status(&mut self) {
        let new_status = self.derive_status();
        if new_status == self.state.status {
            return;
        }
        self.state.status = new_status.clone();
        self.effects.push(Effect::Status(new_status));
    }

    pub fn idle(&self) -> Result<()> {
        anyhow::ensure!(self.ledger.idle(), "agent is busy");
        Ok(())
    }

    fn compacting(&self) -> bool {
        self.ledger.compacting() || self.compaction.is_some()
    }

    fn handle_history(
        &mut self,
        generation: HistoryGeneration,
        event: HistoryUpdate,
    ) -> Result<()> {
        // one clone: the match borrows, then the event moves into the Emit —
        // payloads (resolved tool calls) can embed a full workdir diff
        self.history_mut().handle(generation, event.clone())?;
        match &event {
            HistoryUpdate::TurnResponse(AssistantEvent::Item(item)) => {
                self.run_tool_call(item)?;
            }
            HistoryUpdate::TurnResponse(AssistantEvent::Failed { message, .. }) => {
                tracing::error!("response error: {message}");
            }
            _ => {}
        }
        let skip_save = matches!(
            event,
            HistoryUpdate::GenerationIncremented
                | HistoryUpdate::TurnResponse(AssistantEvent::Delta(_))
        );
        self.effects
            .push(Effect::Ui(UiEvent::HistoryUpdate(generation, event)));
        if skip_save {
            return Ok(());
        }
        // TODO save less often; save on errors
        // every Save in a drain serializes the same post-handle state, so one
        // per drain suffices
        if !self.effects.iter().any(|e| matches!(e, Effect::Save)) {
            self.effects.push(Effect::Save);
        }
        Ok(())
    }

    fn run_tool_call(
        &mut self,
        item: &AssistantItem,
    ) -> Result<()> {
        let AssistantItem::ToolCall(call) = item else {
            return Ok(());
        };
        // also the recursion terminator: resolved calls re-enter
        // handle_history with the output already set
        if call.task.output().is_some() {
            return Ok(());
        }
        // the one capture hook: the core is the only holder of the
        // live history, so `spawn` snapshots it here, at dispatch
        let inherited_history = call
            .task
            .inherit_history()
            .then(|| self.history().subagent());
        self.effects.push(Effect::RunTool {
            id: self.ledger.register(Task::Tool {
                call_id: call.call_id.clone(),
                partial: String::new(),
            }),
            call: call.clone(),
            inherited_history,
        });
        Ok(())
    }

    /// accumulate (authoritative) + tee to the app for live render
    fn output(
        &mut self,
        tid: TaskId,
        chunk: String,
    ) {
        if let Some(Task::Tool { call_id, partial }) = self.ledger.get_mut(tid) {
            partial.push_str(&chunk);
            self.effects.push(Effect::Ui(UiEvent::ToolOutput {
                call_id: call_id.clone(),
                chunk,
            }));
        }
    }

    fn increment_generation(&mut self) -> Result<HistoryGeneration> {
        let generation = self.history().generation();
        self.handle_history(generation, HistoryUpdate::GenerationIncremented)?;
        Ok(self.history().generation())
    }

    /// the single resolver: a task's terminal lands in history, then the
    /// agent continues if that was the last one
    fn task_done(
        &mut self,
        now: u64,
        tid: TaskId,
        result: TaskResult,
    ) -> Result<()> {
        let Some(task) = self.ledger.finish(tid) else {
            // stale (aborted) failures still surface
            if let Err(err) = result {
                self.effects.push(Effect::Ui(UiEvent::Error(err)));
            }
            return Ok(());
        };
        let g = self.history().generation();
        match (task, result) {
            (Task::Turn { .. }, Ok(_)) => {}
            // an erroring or panicking turn terminates its response, so the
            // next flush can't stack a turn on an orphaned InProgress one
            (Task::Turn { generation }, Err(message)) => {
                self.effects
                    .push(Effect::Ui(UiEvent::Error(message.clone())));
                let failed = AssistantEvent::Failed {
                    message,
                    ended_at: now,
                };
                self.handle_history(generation, HistoryUpdate::TurnResponse(failed))?;
            }
            // a streaming tool's authoritative text is the partial; its
            // return carries only metadata
            (Task::Tool { partial, .. }, Ok(TaskOutput::Tool(mut item))) => {
                item.task.compose(partial);
                let item = AssistantEvent::Item(Box::new(AssistantItem::ToolCall(*item)));
                self.handle_history(g, HistoryUpdate::TurnResponse(item))?;
            }
            (Task::Tool { call_id, partial }, result) => {
                let marker = result
                    .err()
                    .unwrap_or_else(|| "tool returned no item".into());
                let error = with_partial(marker, &partial);
                self.handle_history(g, HistoryUpdate::ToolCallFailed { call_id, error })?;
            }
            (Task::Compact { n_drop }, Ok(TaskOutput::Summary(summary))) => {
                self.compaction = Some(Compaction { n_drop, summary });
                return self.advance(now);
            }
            // the agent stops on a failed summary: retrying on our own could
            // loop on a persistent error
            (Task::Compact { .. }, result) => {
                let error = result
                    .err()
                    .unwrap_or_else(|| "compaction returned no summary".into());
                self.effects.push(Effect::Ui(UiEvent::Error(error)));
                self.wants_turn = false;
                return Ok(());
            }
        }
        if !self.ledger.in_turn() {
            self.wants_turn |= self.history().state().needs_another_turn();
        }
        self.advance(now)
    }

    /// the turn boundary, a no-op while a turn is in flight: apply the
    /// ready summary, then start the due turn -- unless that would go past
    /// the hard limit while a summary is still coming
    fn advance(
        &mut self,
        now: u64,
    ) -> Result<()> {
        if self.ledger.in_turn() {
            return Ok(());
        }
        if let Some(compaction) = self.compaction.take() {
            let g = self.history().generation();
            self.handle_history(g, HistoryUpdate::Compact(compaction))?;
        }
        if !self.wants_turn {
            return Ok(());
        }
        self.flush_pending()?;
        self.autocompact(now)?;
        self.wants_turn = self.compacting() && self.past(self.compact.hard_limit);
        if self.wants_turn {
            return Ok(());
        }
        self.start_turn(now)
    }

    /// the single inbound delivery path: buffered, then flushed at the
    /// turn boundary
    fn deliver(
        &mut self,
        now: u64,
        msg: UserMessage,
    ) -> Result<()> {
        self.state.pending_messages.push(msg);
        self.wants_turn = true;
        self.advance(now)?;
        if !self.state.pending_messages.is_empty() {
            // buffered and saved: survives restart
            self.effects.push(Effect::Save);
        }
        Ok(())
    }

    /// startup wake: an idle agent flushes its saved buffer — a spawn
    /// seed, or messages buffered before a restart — and starts their turn;
    /// otherwise they sit unread until an unrelated event pokes the agent
    pub fn resume(
        &mut self,
        now: u64,
    ) -> (Result<()>, Vec<Effect>) {
        self.wants_turn = !self.state.pending_messages.is_empty();
        let result = self.advance(now);
        self.drain(result)
    }

    /// deliver buffered inbound messages at the current generation
    fn flush_pending(&mut self) -> Result<()> {
        for msg in std::mem::take(&mut self.state.pending_messages) {
            let generation = self.history().generation();
            self.handle_history(generation, HistoryUpdate::UserMessage(msg))?;
        }
        Ok(())
    }

    fn submit(
        &mut self,
        now: u64,
        UserPrompt { text, generation }: UserPrompt,
    ) -> Result<()> {
        // busy: queue instead of reject — pending messages carry no
        // generation and flush into a fresh turn at the next turn boundary,
        // so typed input is never destroyed and doubles as steering
        if !self.ledger.in_turn() && !self.wants_turn {
            let current = self.history().generation();
            // a stale submit is rejected *before* the flush: pending
            // messages carry no generation and must never be dropped as stale
            anyhow::ensure!(
                generation.is_none_or(|g| g == current),
                "history generation mismatch: expected {current}",
            );
            self.increment_generation()?;
        }
        // drains a buffer stranded by abort — abort itself never flushes:
        // a naive flush would auto-start a turn on user abort
        self.deliver(now, UserMessage::new(text, now))
    }

    /// resolve everything in flight right here — the failed turn, and each
    /// tool call with what it streamed so far — then cancel the futures,
    /// whose late events the cleared ledger ignores; a summary is dropped
    /// with them
    fn abort(
        &mut self,
        now: u64,
    ) -> Result<()> {
        let tasks = self.ledger.clear();
        self.compaction = None;
        self.wants_turn = false;
        let g = self.increment_generation()?;
        if self
            .history()
            .state()
            .status()
            .is_some_and(|s| s.failable())
        {
            let failed = AssistantEvent::Failed {
                message: ABORTED_BY_USER.into(),
                ended_at: now,
            };
            self.handle_history(g, HistoryUpdate::TurnResponse(failed))?;
        }
        for task in tasks {
            if let Task::Tool { call_id, partial } = task {
                let error = with_partial(ABORTED_BY_USER.into(), &partial);
                self.handle_history(g, HistoryUpdate::ToolCallFailed { call_id, error })?;
            }
        }
        self.effects.push(Effect::AbortTasks);
        Ok(())
    }

    fn window(&self) -> Option<usize> {
        self.assistants
            .assistant(&self.state.assistant_id)
            .ok()?
            .config
            .window
    }

    /// the history takes up at least `percent` of the context window
    fn past(
        &self,
        percent: usize,
    ) -> bool {
        self.window()
            .is_some_and(|window| self.history().token_count() * 100 >= window * percent)
    }

    /// past the threshold, summarize just enough of the oldest messages to
    /// get down to the target
    fn autocompact(
        &mut self,
        now: u64,
    ) -> Result<()> {
        let Some(window) = self.window() else {
            return Ok(());
        };
        if self.compacting() || !self.past(self.compact.threshold) {
            return Ok(());
        }
        match self
            .history()
            .window_percentage_to_n_msg(window, self.compact.target)
        {
            0 => Ok(()),
            n_drop => self.compact(now, n_drop),
        }
    }

    /// summarize the first `n_drop` messages alongside the turns; the
    /// summary replaces them at the turn boundary
    fn compact(
        &mut self,
        now: u64,
        n_drop: usize,
    ) -> Result<()> {
        anyhow::ensure!(!self.compacting(), "already compacting");
        // a turn in flight still writes to the last message: leave it out
        let len = self.history().state().len();
        let stable = if self.ledger.in_turn() { len - 1 } else { len };
        let n_drop = n_drop.min(stable);
        anyhow::ensure!(n_drop > 0, "nothing to compact");
        let assistant = self.assistants.assistant(&self.state.assistant_id)?;
        self.effects.push(Effect::Summarize {
            id: self.ledger.register(Task::Compact { n_drop }),
            assistant,
            instructions: self.history().instructions().to_string(),
            messages: self.history().compact_input(n_drop, now),
        });
        Ok(())
    }

    fn start_turn(
        &mut self,
        now: u64,
    ) -> Result<()> {
        // resolve the fallible lookup before any history/ledger mutation: a
        // stale assistant id must fail the submit, not wedge the agent busy
        let assistant = self.assistants.assistant(&self.state.assistant_id)?;
        // clone before the Created event appends the queued assistant message
        let messages = self.history().state().messages.clone();
        let generation = self.history().generation();
        let instructions = self.history().instructions().to_string();
        let created = AssistantEvent::Created { created_at: now };
        self.handle_history(generation, HistoryUpdate::TurnResponse(created))?;
        self.effects.push(Effect::StartTurn {
            id: self.ledger.register(Task::Turn { generation }),
            assistant,
            tools: self.tools.clone(),
            instructions,
            messages,
        });
        Ok(())
    }
}

/// a failed tool's error: the marker plus whatever it streamed first
fn with_partial(
    marker: String,
    partial: &str,
) -> String {
    if partial.is_empty() {
        marker
    } else {
        format!("{marker}; partial output:\n{partial}")
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::llm::history::TurnStatus;
    use crate::llm::history::message::CompactMessage;
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
            Self::new(state, pool, CompactConfig::default())
        }
    }

    fn user(command: UserCommand) -> AgentEvent {
        AgentEvent::User(command)
    }

    fn submit(
        text: &str,
        generation: HistoryGeneration,
    ) -> AgentEvent {
        user(UserCommand::Submit(UserPrompt {
            text: text.into(),
            generation: Some(generation),
        }))
    }

    fn message(text: &str) -> AgentEvent {
        AgentEvent::Message(UserMessage::new(text.into(), 5))
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

    fn todo_call(output: Option<std::result::Result<TodoResult, String>>) -> AssistantEvent {
        AssistantEvent::Item(Box::new(AssistantItem::ToolCall(todo_item(output))))
    }

    fn text_output(
        id: &str,
        text: &str,
    ) -> AssistantEvent {
        AssistantEvent::Item(Box::new(AssistantItem::Output(OutputItem {
            id: id.into(),
            content: vec![OutputContent::Text(text.into())],
            token_count: 0,
            started_at: 1,
            ended_at: None,
        })))
    }

    fn user_message(text: &str) -> HistoryUpdate {
        HistoryUpdate::UserMessage(UserMessage::new(text.into(), 0))
    }

    /// core with the given user messages already in history
    fn core_with(texts: &[&str]) -> AgentCore {
        let mut core = AgentCore::fake();
        for text in texts {
            core.history_mut().handle(0, user_message(text)).unwrap();
        }
        core
    }

    /// id of the summary task the effects started
    fn summary_task(effects: &[Effect]) -> TaskId {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::Summarize { id, .. } => Some(*id),
                _ => None,
            })
            .expect("no summary task")
    }

    fn starts_turn(effects: &[Effect]) -> bool {
        effects
            .iter()
            .any(|e| matches!(e, Effect::StartTurn { .. }))
    }

    fn summary(text: &str) -> TaskResult {
        Ok(TaskOutput::Summary(CompactMessage {
            text: text.into(),
            token_count: 0,
            created_at: 1,
            started_at: 2,
            ended_at: 3,
        }))
    }

    /// `words` tokens of filler
    fn filler(words: usize) -> String {
        " word".repeat(words)
    }

    /// two 30k fillers and a short tail: past 50% of the fake window, and
    /// dropping the first filler alone gets under 35%
    fn core_near_full(hard_limit: usize) -> AgentCore {
        let (big1, big2) = (filler(30_000), filler(30_000));
        let mut core = core_with(&[&big1, &big2, "tail"]);
        core.compact = CompactConfig {
            threshold: 50,
            target: 35,
            hard_limit,
        };
        core
    }

    macro_rules! assert_handled {
        ($core:expr, $now:expr, $event:expr, @$snapshot:literal) => {{
            let (result, effects) = $core.handle($now, $event);
            result.unwrap();
            insta::assert_yaml_snapshot!(($core.derive_status(), effects), @$snapshot);
        }};
    }

    macro_rules! assert_rejected {
        ($core:expr, $now:expr, $event:expr, @$snapshot:literal) => {{
            let (result, effects) = $core.handle($now, $event);
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
        - turn: InProgress
          compacting: false
        - - Ui:
              HistoryUpdate:
                - 0
                - GenerationIncremented
          - Ui:
              HistoryUpdate:
                - 1
                - UserMessage:
                    text: hi
                    token_count: 1
                    created_at: 7
          - Save
          - Ui:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Created:
                      created_at: 7
          - StartTurn:
              id: 0
              assistant: test
          - Status:
              turn: InProgress
              compacting: false
        ");
    }

    #[test]
    fn submit_with_stale_generation_is_rejected() {
        let mut core = AgentCore::fake();
        assert_rejected!(&mut core, 7, submit("hi", 1), @r#"
        - "history generation mismatch: expected 0"
        - turn: Idle
          compacting: false
        - []
        "#);
    }

    #[test]
    fn submit_while_busy_queues_as_pending() {
        let mut core = AgentCore::fake();
        core.ledger.register(Task::turn());
        assert_handled!(&mut core, 7, submit("hi", 0), @"
        - turn: InProgress
          compacting: false
        - - Save
          - Status:
              turn: InProgress
              compacting: false
        ");
        assert_eq!(core.state.pending_messages.len(), 1);
    }

    /// a mid-turn submit — even one stamped with a nonsense generation —
    /// is queued and flushes into its own turn at turn end, so typed input
    /// is never destroyed by a busy rejection
    #[test]
    fn busy_submit_flushes_into_a_turn_at_turn_end() {
        let mut core = AgentCore::fake();
        let (_, effects) = core.handle(1, submit("hi", 0));
        let turn = started_task(&effects);
        core.handle(2, submit("steer", 999)).0.unwrap();
        assert_eq!(core.state.pending_messages.len(), 1);

        core.handle(
            3,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 3 }),
        )
        .0
        .unwrap();
        let (result, effects) = core.handle(4, AgentEvent::Done(turn, Ok(TaskOutput::Turn)));
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
        core.state.assistant_id = "gone".into();
        let (result, effects) = core.handle(7, submit("hi", 0));
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
        core.handle(8, user(UserCommand::SetAssistant("test".into())))
            .0
            .unwrap();
        core.state.assistant_id = "test".into();
        core.handle(9, submit("retry", 1)).0.unwrap();
        similar_asserts::assert_eq!(
            core.derive_status(),
            ActivityStatus {
                turn: TurnStatus::InProgress,
                compacting: false,
            }
        );
    }

    #[test]
    fn abort_fails_inflight_turn() {
        let mut core = AgentCore::fake();
        core.handle(7, submit("hi", 0)).0.unwrap();
        assert_handled!(&mut core, 9, user(UserCommand::Abort), @"
        - turn:
            Failed: aborted by user
          compacting: false
        - - Ui:
              HistoryUpdate:
                - 1
                - GenerationIncremented
          - Ui:
              HistoryUpdate:
                - 2
                - TurnResponse:
                    Failed:
                      message: aborted by user
                      ended_at: 9
          - Save
          - AbortTasks
          - Status:
              turn:
                Failed: aborted by user
              compacting: false
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
    fn task_failure_emits_error_and_keeps_failed_status() {
        let mut core = AgentCore::fake();
        core.state.status = ActivityStatus {
            turn: TurnStatus::InProgress,
            compacting: false,
        };
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
        let tid = core.ledger.register(Task::turn());

        assert_handled!(&mut core, 7, AgentEvent::Done(tid, Err("oops".into())), @"
        - turn:
            Failed: oops
          compacting: false
        - - Ui:
              Error: oops
          - Ui:
              HistoryUpdate:
                - 0
                - TurnResponse:
                    Failed:
                      message: oops
                      ended_at: 7
          - Save
          - Status:
              turn:
                Failed: oops
              compacting: false
        ");
    }

    #[test]
    fn set_assistant_rejected_while_busy() {
        let mut core = AgentCore::fake();
        core.ledger.register(Task::turn());
        assert_rejected!(&mut core, 7, user(UserCommand::SetAssistant("test2".into())), @"
        - agent is busy
        - turn: InProgress
          compacting: false
        - []
        ");
        assert_eq!(core.state.assistant_id, "test");
    }

    #[test]
    fn set_assistant_resolves_to_single_effect() {
        let mut core = AgentCore::fake();
        assert_handled!(&mut core, 7, user(UserCommand::SetAssistant("test2".into())), @"
        - turn: Idle
          compacting: false
        - - SetAssistant: test2
        ");
        // core state untouched: `Agent` applies it after the save succeeds
        assert_eq!(core.state.assistant_id, "test");
    }

    #[test]
    fn set_assistant_unknown_id_is_rejected() {
        let mut core = AgentCore::fake();
        assert_rejected!(&mut core, 7, user(UserCommand::SetAssistant("nope".into())), @r#"
        - "unknown assistant \"nope\""
        - turn: Idle
          compacting: false
        - []
        "#);
    }

    #[test]
    fn abort_while_idle_emits_no_history_event() {
        let mut core = AgentCore::fake();
        assert_handled!(&mut core, 9, user(UserCommand::Abort), @"
        - turn: Idle
          compacting: false
        - - Ui:
              HistoryUpdate:
                - 0
                - GenerationIncremented
          - AbortTasks
        ");
    }

    #[test]
    fn task_failure_after_abort_surfaces_error_without_reply() {
        let mut core = AgentCore::fake();
        let (_, effects) = core.handle(1, submit("hi", 0));
        let tid = started_task(&effects);
        core.handle(2, user(UserCommand::Abort)).0.unwrap();

        assert_handled!(&mut core, 3, AgentEvent::Done(tid, Err("stream closed".into())), @"
        - turn:
            Failed: aborted by user
          compacting: false
        - - Ui:
              Error: stream closed
        ");
    }

    #[test]
    fn task_event_after_abort_is_dropped() {
        let mut core = AgentCore::fake();
        let (_, effects) = core.handle(1, submit("hi", 0));
        let tid = started_task(&effects);
        core.handle(2, user(UserCommand::Abort)).0.unwrap();

        assert_handled!(&mut core, 3, AgentEvent::Stream(tid, text_output("out", "late")), @"
        - turn:
            Failed: aborted by user
          compacting: false
        - []
        ");
    }

    #[test]
    fn task_done_starts_followup_turn_after_tool_resolves() {
        let mut core = AgentCore::fake();
        let (_, effects) = core.handle(1, submit("hi", 0));
        let turn = started_task(&effects);

        let (result, effects) = core.handle(2, AgentEvent::Stream(turn, todo_call(None)));
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
        - Ui:
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

        core.handle(
            3,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 3 }),
        )
        .0
        .unwrap();
        // the turn task finishing leaves the tool task pending: no new turn yet
        let (result, effects) = core.handle(4, AgentEvent::Done(turn, Ok(TaskOutput::Turn)));
        result.unwrap();
        assert!(effects.is_empty());

        // the reaper's terminal resolves the slot and starts the follow-up
        assert_handled!(
            &mut core, 6,
            AgentEvent::Done(tool, Ok(TaskOutput::Tool(Box::new(todo_item(Some(Ok(TodoResult {}))))))),
            @r#"
        - turn: InProgress
          compacting: false
        - - Ui:
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
          - Ui:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Created:
                      created_at: 6
          - StartTurn:
              id: 2
              assistant: test
        "#);
    }

    #[test]
    fn reaped_tool_failure_patches_slot_and_starts_followup() {
        let mut core = AgentCore::fake();
        let (_, effects) = core.handle(1, submit("hi", 0));
        let turn = started_task(&effects);
        let (_, effects) = core.handle(2, AgentEvent::Stream(turn, todo_call(None)));
        let tool = started_task(&effects);
        core.handle(
            3,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 3 }),
        )
        .0
        .unwrap();
        core.handle(4, AgentEvent::Done(turn, Ok(TaskOutput::Turn)))
            .0
            .unwrap();

        core.handle(5, AgentEvent::Output(tool, "abc".into()))
            .0
            .unwrap();
        // a panic finalizes: slot gets the error plus the streamed partial,
        // ledger unsticks, and the model sees the failure next turn
        assert_handled!(
            &mut core, 5,
            AgentEvent::Done(tool, Err("tool panicked: boom".into())),
            @r#"
        - turn: InProgress
          compacting: false
        - - Ui:
              HistoryUpdate:
                - 1
                - ToolCallFailed:
                    call_id: call-1
                    error: "tool panicked: boom; partial output:\nabc"
          - Save
          - Ui:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Created:
                      created_at: 5
          - StartTurn:
              id: 2
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
        - turn: InProgress
          compacting: false
        - - Ui:
              HistoryUpdate:
                - 0
                - UserMessage:
                    text: "[from: kid]\nhi"
                    token_count: 5
                    created_at: 5
          - Save
          - Ui:
              HistoryUpdate:
                - 0
                - TurnResponse:
                    Created:
                      created_at: 7
          - StartTurn:
              id: 0
              assistant: test
          - Status:
              turn: InProgress
              compacting: false
        "#);
        assert!(core.state.pending_messages.is_empty());
    }

    #[test]
    fn message_while_busy_buffers_then_flushes_on_idle() {
        let mut core = AgentCore::fake();
        let (_, effects) = core.handle(1, submit("hi", 0));
        let turn = started_task(&effects);

        // turn still streaming: buffered, save only
        assert_handled!(&mut core, 2, message("[from: kid]\nearly bird"), @"
        - turn: InProgress
          compacting: false
        - - Save
        ");
        assert_eq!(core.state.pending_messages.len(), 1);

        core.handle(
            3,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 3 }),
        )
        .0
        .unwrap();
        // idle: the buffer flushes and wakes the agent
        assert_handled!(&mut core, 4, AgentEvent::Done(turn, Ok(TaskOutput::Turn)), @r#"
        - turn: InProgress
          compacting: false
        - - Ui:
              HistoryUpdate:
                - 1
                - UserMessage:
                    text: "[from: kid]\nearly bird"
                    token_count: 6
                    created_at: 5
          - Save
          - Ui:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Created:
                      created_at: 4
          - StartTurn:
              id: 1
              assistant: test
        "#);
        assert!(core.state.pending_messages.is_empty());
    }

    /// a message that can't start its turn still lands in history and is
    /// saved; the error goes to the caller, and the agent stays idle
    #[test]
    fn failed_wake_keeps_the_message() {
        let mut core = AgentCore::fake();
        core.state.assistant_id = "gone".into();
        assert_rejected!(&mut core, 5, message("[from: kid]\nhi"), @r#"
        - "unknown assistant \"gone\""
        - turn: Idle
          compacting: false
        - - Ui:
              HistoryUpdate:
                - 0
                - UserMessage:
                    text: "[from: kid]\nhi"
                    token_count: 5
                    created_at: 5
          - Save
        "#);
    }

    #[test]
    fn stale_submit_is_rejected_before_flush() {
        let mut core = AgentCore::fake();
        core.handle(1, submit("hi", 0)).0.unwrap();
        core.handle(2, message("[from: kid]\nstranded")).0.unwrap();
        // abort clears the ledger but deliberately never flushes
        core.handle(3, user(UserCommand::Abort)).0.unwrap();
        assert_eq!(core.state.pending_messages.len(), 1);

        // stale generation: rejected before the flush — the buffer is intact
        let (result, effects) = core.handle(4, submit("late", 1));
        assert!(result.is_err());
        assert!(effects.is_empty());
        assert_eq!(core.state.pending_messages.len(), 1);
    }

    #[test]
    fn submit_flushes_stranded_buffer_before_user_message() {
        let mut core = AgentCore::fake();
        core.handle(1, submit("hi", 0)).0.unwrap();
        core.handle(2, message("[from: kid]\nstranded")).0.unwrap();
        core.handle(3, user(UserCommand::Abort)).0.unwrap();

        let (result, effects) = core.handle(4, submit("continue", 2));
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
        core.handle(1, submit("hi", 0)).0.unwrap();
        core.handle(2, message("[from: kid]\npersisted")).0.unwrap();
        core.handle(3, user(UserCommand::Abort)).0.unwrap();

        // simulated restart: rebuild the core from the persisted state
        let json = serde_json::to_value(&core.state).unwrap();
        let restored: AgentState = serde_json::from_value(json).unwrap();
        let mut restored = AgentCore::new(restored, core.assistants.clone(), core.compact);
        assert_eq!(restored.state.pending_messages.len(), 1);

        // resume (the startup wake) delivers the buffered message and starts
        // its turn — no submit or other poke needed
        let (result, effects) = restored.resume(5);
        result.unwrap();
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
        core.handle(1, submit("hi", 0)).0.unwrap();
        core.handle(2, message("[from: kid]\npersisted")).0.unwrap();
        core.handle(3, user(UserCommand::Abort)).0.unwrap();

        let json = serde_json::to_value(&core.state).unwrap();
        let restored: AgentState = serde_json::from_value(json).unwrap();
        let mut restored = AgentCore::new(restored, core.assistants.clone(), core.compact);
        assert_eq!(restored.state.pending_messages.len(), 1);

        // restored generation resets to 0: it is #[serde(skip)]
        let (result, effects) = restored.handle(5, submit("go", 0));
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
        let (_, effects) = core.handle(1, submit("hi", 0));
        let turn = started_task(&effects);

        let capture_of = |core: &mut AgentCore, item: ToolCallItem| {
            let item = AssistantEvent::Item(Box::new(AssistantItem::ToolCall(item)));
            let (result, effects) = core.handle(2, AgentEvent::Stream(turn, item));
            result.unwrap();
            effects
                .into_iter()
                .find_map(|e| match e {
                    Effect::RunTool {
                        inherited_history, ..
                    } => Some(inherited_history),
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
                    commit: None,
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
        let captured = capture_of(&mut core, spawn_item("call-1", true)).unwrap();
        assert!(
            captured
                .state()
                .messages
                .iter()
                .any(|m| matches!(m, Message::User(u) if u.text == "hi"))
        );
        assert_eq!(captured.generation(), 0);
        // inherit=false: nothing to capture, the child starts fresh
        assert!(capture_of(&mut core, spawn_item("call-2", false)).is_none());
        // non-spawn tools carry no capture
        assert!(capture_of(&mut core, todo_item(None)).is_none());
    }

    #[test]
    fn task_done_returns_to_idle() {
        let mut core = AgentCore::fake();
        let (_, effects) = core.handle(1, submit("hi", 0));
        let tid = started_task(&effects);
        core.handle(2, AgentEvent::Stream(tid, text_output("out", "done")))
            .0
            .unwrap();
        core.handle(
            3,
            AgentEvent::Stream(tid, AssistantEvent::Completed { ended_at: 3 }),
        )
        .0
        .unwrap();

        assert_handled!(&mut core, 4, AgentEvent::Done(tid, Ok(TaskOutput::Turn)), @"
        - turn: Idle
          compacting: false
        - - Status:
              turn: Idle
              compacting: false
        ");
    }

    #[test]
    fn task_done_failure_surfaces_error_and_failed_status() {
        let mut core = AgentCore::fake();
        let (_, effects) = core.handle(1, submit("hi", 0));
        let tid = started_task(&effects);
        core.handle(
            2,
            AgentEvent::Stream(
                tid,
                AssistantEvent::Failed {
                    message: "boom".into(),
                    ended_at: 2,
                },
            ),
        )
        .0
        .unwrap();

        assert_handled!(&mut core, 3, AgentEvent::Done(tid, Err("boom".into())), @"
        - turn:
            Failed: boom
          compacting: false
        - - Ui:
              Error: boom
          - Ui:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Failed:
                      message: boom
                      ended_at: 3
          - Save
          - Status:
              turn:
                Failed: boom
              compacting: false
        ");
    }

    #[test]
    fn resolved_tool_call_is_not_rerun() {
        let mut core = AgentCore::fake();
        let (_, effects) = core.handle(1, submit("hi", 0));
        let tid = started_task(&effects);

        assert_handled!(
            &mut core, 2,
            AgentEvent::Stream(tid, todo_call(Some(Ok(TodoResult {})))),
            @r#"
        - turn: InProgress
          compacting: false
        - - Ui:
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

        assert_handled!(&mut core, 7, user(UserCommand::Retry), @"
        - turn: InProgress
          compacting: false
        - - Ui:
              HistoryUpdate:
                - 0
                - GenerationIncremented
          - Ui:
              HistoryUpdate:
                - 1
                - TurnResponse:
                    Created:
                      created_at: 7
          - Save
          - StartTurn:
              id: 0
              assistant: test
          - Status:
              turn: InProgress
              compacting: false
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

        assert_handled!(&mut core, 7, user(UserCommand::Undo(1)), @"
        - turn: Idle
          compacting: false
        - - Ui:
              HistoryUpdate:
                - 0
                - GenerationIncremented
          - Ui:
              HistoryUpdate:
                - 1
                - Pop: 1
          - Save
        ");
        assert_eq!(core.history().state().messages.len(), 1);
    }

    #[test]
    fn compact_runs_alongside_turn_and_applies_at_turn_end() {
        let mut core = core_with(&["first"]);
        let (_, effects) = core.handle(1, submit("hi", 0));
        let turn = started_task(&effects);

        // the in-flight assistant message is left out of the summary
        let (result, effects) = core.handle(2, user(UserCommand::Compact(99)));
        result.unwrap();
        let compact = summary_task(&effects);
        insta::assert_yaml_snapshot!((core.derive_status(), effects), @r#"
        - turn: InProgress
          compacting: true
        - - Summarize:
              id: 1
              assistant: test
              messages:
                - role: user
                  text: first
                  token_count: 1
                  created_at: 0
                - role: user
                  text: hi
                  token_count: 1
                  created_at: 1
                - role: user
                  text: "Summarize this conversation for future continuation. Keep concrete user requirements, decisions, constraints, file paths, and unresolved work. Be concise and factual. Output plain text only."
                  token_count: 35
                  created_at: 2
          - Status:
              turn: InProgress
              compacting: true
        "#);

        // the summary lands mid-turn: held until the turn ends
        assert_handled!(&mut core, 3, AgentEvent::Done(compact, summary("gist")), @"
        - turn: InProgress
          compacting: true
        - []
        ");
        core.handle(4, AgentEvent::Stream(turn, text_output("out", "done")))
            .0
            .unwrap();
        core.handle(
            5,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 5 }),
        )
        .0
        .unwrap();
        assert_handled!(&mut core, 6, AgentEvent::Done(turn, Ok(TaskOutput::Turn)), @"
        - turn: Idle
          compacting: false
        - - Ui:
              HistoryUpdate:
                - 1
                - Compact:
                    n_drop: 2
                    summary:
                      text: gist
                      token_count: 0
                      created_at: 1
                      started_at: 2
                      ended_at: 3
          - Save
          - Status:
              turn: Idle
              compacting: false
        ");
        insta::assert_yaml_snapshot!(core.history().state().messages, @"
        - role: developer
          Compact:
            text: gist
            token_count: 1
            created_at: 1
            started_at: 2
            ended_at: 3
        - role: assistant
          status: Success
          content:
            - - out
              - Output:
                  id: out
                  content:
                    - Text: done
                  token_count: 1
                  started_at: 1
                  ended_at: ~
          token_count: 1
          created_at: 1
          started_at: ~
          ended_at: 5
          ready_at: ~
        ");
    }

    #[test]
    fn compact_while_idle_applies_on_arrival() {
        let mut core = core_with(&["first", "second"]);
        let (_, effects) = core.handle(1, user(UserCommand::Compact(1)));
        let compact = summary_task(&effects);

        assert_handled!(&mut core, 2, AgentEvent::Done(compact, summary("gist")), @"
        - turn: Idle
          compacting: false
        - - Ui:
              HistoryUpdate:
                - 0
                - Compact:
                    n_drop: 1
                    summary:
                      text: gist
                      token_count: 0
                      created_at: 1
                      started_at: 2
                      ended_at: 3
          - Save
          - Status:
              turn: Idle
              compacting: false
        ");
        assert_eq!(core.history().state().messages.len(), 2);
    }

    #[test]
    fn submit_while_compacting_starts_turn_right_away() {
        let mut core = core_with(&["first"]);
        core.handle(1, user(UserCommand::Compact(1))).0.unwrap();

        let (result, effects) = core.handle(2, submit("hi", 0));
        result.unwrap();
        assert!(starts_turn(&effects));
        assert!(core.ledger.in_turn() && core.ledger.compacting());
    }

    #[test]
    fn commands_rejected_while_compacting() {
        let mut core = core_with(&["first", "second"]);
        core.handle(1, user(UserCommand::Compact(1))).0.unwrap();

        assert_rejected!(&mut core, 2, user(UserCommand::Compact(1)), @"
        - already compacting
        - turn: Idle
          compacting: true
        - []
        ");
        assert_rejected!(&mut core, 3, user(UserCommand::Undo(1)), @"
        - agent is busy
        - turn: Idle
          compacting: true
        - []
        ");
    }

    #[test]
    fn compact_zero_messages_is_rejected() {
        let mut core = core_with(&["short"]);
        assert_rejected!(&mut core, 7, user(UserCommand::Compact(0)), @"
        - nothing to compact
        - turn: Idle
          compacting: false
        - []
        ");
        assert!(core.ledger.idle());
    }

    #[test]
    fn abort_drops_compaction() {
        let mut core = core_with(&["first"]);
        core.handle(1, submit("hi", 0)).0.unwrap();
        let (_, effects) = core.handle(2, user(UserCommand::Compact(1)));
        let compact = summary_task(&effects);

        core.handle(3, user(UserCommand::Abort)).0.unwrap();
        assert!(!core.derive_status().compacting);

        // the late summary is stale
        assert_handled!(&mut core, 4, AgentEvent::Done(compact, summary("gist")), @"
        - turn:
            Failed: aborted by user
          compacting: false
        - []
        ");
        assert!(core.compaction.is_none());
    }

    /// abort strands the buffer on purpose: a summary landing later must
    /// not flush it into a turn the user didn't ask for
    #[test]
    fn summary_after_abort_keeps_stranded_buffer() {
        let mut core = core_with(&["first"]);
        core.handle(1, submit("hi", 0)).0.unwrap();
        core.handle(2, message("[from: kid]\nstranded")).0.unwrap();
        core.handle(3, user(UserCommand::Abort)).0.unwrap();
        let (_, effects) = core.handle(4, user(UserCommand::Compact(1)));
        let compact = summary_task(&effects);

        let (result, effects) = core.handle(5, AgentEvent::Done(compact, summary("gist")));
        result.unwrap();

        assert!(!starts_turn(&effects));
        assert_eq!(core.state.pending_messages.len(), 1);
        assert!(core.ledger.idle());
    }

    #[test]
    fn autocompact_past_threshold_runs_alongside_the_turn() {
        let mut core = core_near_full(100);

        let (result, effects) = core.handle(1, submit("hi", 0));
        result.unwrap();

        let compact = summary_task(&effects);
        assert!(matches!(
            core.ledger.get(compact),
            Some(Task::Compact { n_drop: 1 })
        ));
        assert!(starts_turn(&effects));
    }

    #[test]
    fn autocompact_below_threshold_does_nothing() {
        let mut core = core_near_full(100);
        core.compact.threshold = 90;

        let (result, effects) = core.handle(1, submit("hi", 0));
        result.unwrap();

        assert!(!core.ledger.compacting());
        assert!(starts_turn(&effects));
    }

    #[test]
    fn hard_limit_holds_turn_until_summary_lands() {
        let mut core = core_near_full(50);

        let (result, effects) = core.handle(1, submit("hi", 0));
        result.unwrap();
        let compact = summary_task(&effects);
        assert!(!starts_turn(&effects));
        // the prompt is in history, its turn is held
        assert!(matches!(
            core.history().state().last(),
            Some(Message::User(UserMessage { text, .. })) if text == "hi"
        ));
        insta::assert_yaml_snapshot!(core.derive_status(), @"
        turn: InProgress
        compacting: true
        ");

        // held = busy: a second prompt lands without starting a turn either
        let (result, effects) = core.handle(2, submit("more", 999));
        result.unwrap();
        assert!(!starts_turn(&effects));

        let (result, effects) = core.handle(3, AgentEvent::Done(compact, summary("gist")));
        result.unwrap();
        assert!(starts_turn(&effects));
        assert!(!core.derive_status().compacting);
        insta::assert_yaml_snapshot!(
            core.history().state().messages,
            {
                "[0].Compact.token_count" => "[tokens]",
                "[1].text" => "[filler]",
            },
            @r#"
        - role: developer
          Compact:
            text: gist
            token_count: "[tokens]"
            created_at: 1
            started_at: 2
            ended_at: 3
        - role: user
          text: "[filler]"
          token_count: 30000
          created_at: 0
        - role: user
          text: tail
          token_count: 1
          created_at: 0
        - role: user
          text: hi
          token_count: 1
          created_at: 1
        - role: user
          text: more
          token_count: 1
          created_at: 2
        - role: assistant
          status: Queued
          content: []
          token_count: 0
          created_at: 3
          started_at: ~
          ended_at: ~
          ready_at: ~
        "#
        );
    }

    #[test]
    fn failed_summary_stops_held_turn() {
        let mut core = core_near_full(50);
        let (_, effects) = core.handle(1, submit("hi", 0));
        let compact = summary_task(&effects);

        assert_handled!(&mut core, 2, AgentEvent::Done(compact, Err("rate limited".into())), @"
        - turn: Idle
          compacting: false
        - - Ui:
              Error: rate limited
          - Status:
              turn: Idle
              compacting: false
        ");

        // retry asks for a fresh summary, and holds the turn again
        let (result, effects) = core.handle(3, user(UserCommand::Retry));
        result.unwrap();
        summary_task(&effects);
        assert!(!starts_turn(&effects));
    }
}

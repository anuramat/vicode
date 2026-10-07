//! agent event handling: every [`AgentEvent`] is one step that updates the
//! state, spawns tasks and emits UI events right away; the state is saved
//! once, at the end of the step

use anyhow::Result;
use tracing::debug;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::event::AgentEvent;
use crate::agent::event::TaskOutput;
use crate::agent::event::TaskResult;
use crate::agent::event::UiEvent;
use crate::agent::event::UserCommand;
use crate::agent::event::UserPrompt;
use crate::agent::router::graph::NodeStatus;
use crate::agent::task::ledger::Task;
use crate::agent::task::ledger::TaskId;
use crate::agent::task::sink::TaskSink;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::agent::tool::registry::TOOL_REGISTRY;
use crate::llm::history::AssistantEvent;
use crate::llm::history::Compaction;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::UserMessage;

pub const ABORTED_BY_USER: &str = "aborted by user";
pub const LOST_ON_RESTART: &str = "lost on restart";

impl Agent {
    #[instrument(skip(self))]
    pub async fn handle(
        &mut self,
        now: u64,
        event: AgentEvent,
    ) -> Result<()> {
        debug!(event = ?event, "handling agent event");
        let result = match event {
            AgentEvent::User(command) => self.command(now, command).await,
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
        self.settle(result).await
    }

    /// startup wake: an idle agent flushes its saved buffer — a spawn
    /// seed, or messages buffered before a restart — and starts their turn;
    /// otherwise they sit unread until an unrelated event pokes the agent
    pub async fn resume(
        &mut self,
        now: u64,
    ) -> Result<()> {
        self.needs_turn = !self.state.pending_messages.is_empty();
        let result = self.advance(now);
        self.settle(result).await
    }

    /// end of a step: report the status if it succeeded, and save what it
    /// changed even if it failed; the first error wins
    async fn settle(
        &mut self,
        result: Result<()>,
    ) -> Result<()> {
        if result.is_ok() {
            self.report_status();
        }
        if !std::mem::take(&mut self.dirty) {
            return result;
        }
        let saved = self.save().await;
        result.and(saved)
    }

    async fn command(
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
                self.needs_turn = true;
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
                let id = self.project.assistants().assistant(&id)?.id;
                self.emit(UiEvent::AssistantSet(id.clone()));
                self.state.assistant_id = id;
                self.dirty = true;
                Ok(())
            }
            // success needs no reply: the copy's own Started event attaches
            // the app's preview tab; the rejection names the copy, so the
            // app can drop its preview
            UserCommand::Duplicate(copy) => {
                if let Err(e) = self.try_duplicate(copy.clone()).await {
                    let error = format!("{e:#}");
                    self.emit(UiEvent::DuplicateFailed { copy, error });
                }
                Ok(())
            }
        }
    }

    /// a turn or compaction is in flight or held
    pub fn busy(&self) -> bool {
        !self.ledger.idle() || self.needs_turn || self.compaction.is_some()
    }

    /// informational: the router's `list`
    pub fn report_status(&self) {
        let status = if self.busy() {
            NodeStatus::Running
        } else {
            NodeStatus::Idle
        };
        self.router.report_status(&self.id, status);
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
        // one clone: the match borrows, then the event moves into the emit —
        // payloads (resolved tool calls) can embed a full workdir diff
        self.history_mut().handle(generation, event.clone())?;
        match &event {
            HistoryUpdate::TurnResponse(AssistantEvent::Item(item)) => self.run_tool_call(item),
            HistoryUpdate::TurnResponse(AssistantEvent::Failed { message, .. }) => {
                tracing::error!("response error: {message}");
            }
            _ => {}
        }
        // TODO save less often; save on errors
        self.dirty |= !matches!(
            event,
            HistoryUpdate::GenerationIncremented
                | HistoryUpdate::TurnResponse(AssistantEvent::Delta(_))
        );
        self.emit(UiEvent::HistoryUpdate(generation, event));
        Ok(())
    }

    fn run_tool_call(
        &mut self,
        item: &AssistantItem,
    ) {
        let AssistantItem::ToolCall(call) = item else {
            return;
        };
        // also the recursion terminator: resolved calls re-enter
        // handle_history with the output already set
        if call.task.output().is_some() {
            return;
        }
        // the one capture hook: the agent is the only holder of the
        // live history, so `spawn` snapshots it here, at dispatch
        let inherited_history = call
            .task
            .inherit_history()
            .then(|| self.history().subagent());
        let id = self.ledger.register(Task::Tool {
            call_id: call.call_id.clone(),
            partial: String::new(),
        });
        let ctx = ToolRuntimeContext::new(
            self.id.clone(),
            self.project.clone(),
            self.router.clone(),
            TaskSink::new(id, self.task_tx.clone()),
            inherited_history,
        );
        let mut call = call.clone();
        self.executor.spawn(id, "tool", async move {
            call.task.run(ctx).await;
            call.touch_ready_at_now();
            Ok(TaskOutput::Tool(Box::new(call)))
        });
    }

    /// accumulate (authoritative) + tee to the app for live render
    fn output(
        &mut self,
        tid: TaskId,
        chunk: String,
    ) {
        if let Some(Task::Tool { call_id, partial }) = self.ledger.get_mut(tid) {
            partial.push_str(&chunk);
            let call_id = call_id.clone();
            self.emit(UiEvent::ToolOutput { call_id, chunk });
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
                self.emit(UiEvent::Error(err));
            }
            return Ok(());
        };
        let g = self.history().generation();
        match (task, result) {
            (Task::Turn { .. }, Ok(_)) => {}
            // an erroring or panicking turn terminates its response, so the
            // next flush can't stack a turn on an orphaned InProgress one
            (Task::Turn { generation }, Err(message)) => {
                self.emit(UiEvent::Error(message.clone()));
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
                self.emit(UiEvent::Error(error));
                self.needs_turn = false;
                return Ok(());
            }
        }
        if !self.ledger.in_turn() {
            self.needs_turn |= self.history().state().ends_with_tool_calls();
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
        if !self.needs_turn {
            return Ok(());
        }
        self.flush_pending()?;
        self.autocompact(now)?;
        self.needs_turn = self.compacting() && self.past(self.project.config().compact.hard_limit);
        if self.needs_turn {
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
        // buffered and saved: survives restart
        self.state.pending_messages.push(msg);
        self.dirty = true;
        self.needs_turn = true;
        self.advance(now)
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
        if !self.ledger.in_turn() && !self.needs_turn {
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
        // an abort keeps everything its tools streamed before it: apply the
        // queued output first (queued turn events die with the turn anyway)
        for _ in 0..self.task_rx.len() {
            if let Ok(AgentEvent::Output(tid, chunk)) = self.task_rx.try_recv() {
                self.output(tid, chunk);
            }
        }
        let tasks = self.ledger.clear();
        self.compaction = None;
        self.needs_turn = false;
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
        self.executor.abort_all();
        Ok(())
    }

    fn window(&self) -> Option<usize> {
        self.project
            .assistants()
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
        let config = self.project.config().compact;
        if self.compacting() || !self.past(config.threshold) {
            return Ok(());
        }
        match self
            .history()
            .window_percentage_to_n_msg(window, config.target)
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
        let assistant = self
            .project
            .assistants()
            .assistant(&self.state.assistant_id)?;
        let instructions = self.history().instructions().to_string();
        let messages = self.history().compact_input(n_drop, now);
        let id = self.ledger.register(Task::Compact { n_drop });
        self.executor.spawn(id, "summary", async move {
            Self::summarize(&assistant, instructions, messages)
                .await
                .map(TaskOutput::Summary)
                .map_err(|e| e.to_string())
        });
        Ok(())
    }

    fn start_turn(
        &mut self,
        now: u64,
    ) -> Result<()> {
        // resolve the fallible lookup before any history/ledger mutation: a
        // stale assistant id must fail the submit, not wedge the agent busy
        let assistant = self
            .project
            .assistants()
            .assistant(&self.state.assistant_id)?;
        // clone before the Created event appends the queued assistant message
        let messages = self.history().state().messages.clone();
        let generation = self.history().generation();
        let instructions = self.history().instructions().to_string();
        let created = AssistantEvent::Created { created_at: now };
        self.handle_history(generation, HistoryUpdate::TurnResponse(created))?;
        let id = self.ledger.register(Task::Turn { generation });
        let sink = TaskSink::new(id, self.task_tx.clone());
        self.executor.spawn(id, "turn", async move {
            Self::turn(
                sink,
                &assistant,
                TOOL_REGISTRY.clone(),
                instructions,
                messages,
            )
            .await
            .map(|()| TaskOutput::Turn)
            .map_err(|e| e.to_string())
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
    use derive_more::Deref;
    use derive_more::DerefMut;
    use similar_asserts::assert_eq;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::agent::AgentId;
    use crate::agent::task::executor::TaskExecutor;
    use crate::config::CompactConfig;
    use crate::llm::history::message::CompactMessage;
    use crate::llm::history::message::Message;
    use crate::llm::history::message::OutputContent;
    use crate::llm::history::message::OutputItem;
    use crate::llm::history::message::ToolCallItem;
    use crate::tools::todo::TodoArguments;
    use crate::tools::todo::TodoCall;
    use crate::tools::todo::TodoResult;
    use crate::tui::app::AppEvent;

    /// an agent whose tasks are recorded, never run: tests feed each task's
    /// events by hand
    #[derive(Deref, DerefMut)]
    struct Harness {
        #[deref]
        #[deref_mut]
        agent: Agent,
        app_rx: UnboundedReceiver<AppEvent>,
    }

    /// what a step did
    #[derive(serde::Serialize)]
    struct Step {
        busy: bool,
        ui: Vec<UiEvent>,
        /// started tasks, as (id, kind)
        tasks: Vec<(TaskId, &'static str)>,
    }

    impl Harness {
        /// on the fake assistant pool ("test" + "test2"), with the given
        /// user messages already in history
        async fn with(texts: &[&str]) -> Self {
            let (agent, _api, app_rx) = Agent::fake("handle").await;
            let mut h = Self::wrap(agent, app_rx);
            for text in texts {
                h.history_mut().handle(0, user_message(text)).unwrap();
            }
            h
        }

        fn wrap(
            mut agent: Agent,
            app_rx: UnboundedReceiver<AppEvent>,
        ) -> Self {
            agent.executor = TaskExecutor::Held(Vec::new());
            Self { agent, app_rx }
        }

        /// a fresh agent on the persisted state, as after a restart
        async fn restart(&self) -> Self {
            let state = self.project.store().load_state(&self.id).await.unwrap();
            let (app_tx, app_rx) = unbounded_channel();
            let agent = Agent::new(
                self.project.clone(),
                self.router.clone(),
                app_tx,
                self.id.clone(),
                state,
            );
            Self::wrap(agent, app_rx)
        }

        async fn step(
            &mut self,
            now: u64,
            event: AgentEvent,
        ) -> (Result<()>, Step) {
            let result = self.agent.handle(now, event).await;
            (result, self.drain())
        }

        /// what happened since the last drain
        fn drain(&mut self) -> Step {
            let mut ui = Vec::new();
            while let Ok(AppEvent::Agent(_, event)) = self.app_rx.try_recv() {
                ui.push(event);
            }
            let TaskExecutor::Held(tasks) = &mut self.agent.executor else {
                unreachable!("the harness holds its tasks");
            };
            let tasks = std::mem::take(tasks);
            Step {
                busy: self.agent.busy(),
                ui,
                tasks,
            }
        }
    }

    impl Step {
        /// id of the first task the step started
        fn task(&self) -> TaskId {
            self.tasks.first().expect("no task started").0
        }

        /// id of the summary task the step started
        fn summary(&self) -> TaskId {
            self.tasks
                .iter()
                .find_map(|&(id, kind)| (kind == "summary").then_some(id))
                .expect("no summary task")
        }

        fn starts_turn(&self) -> bool {
            self.tasks.iter().any(|&(_, kind)| kind == "turn")
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
    async fn near_full(hard_limit: usize) -> Harness {
        let (big1, big2) = (filler(30_000), filler(30_000));
        let mut h = Harness::with(&[&big1, &big2, "tail"]).await;
        h.project.config_mut().compact = CompactConfig {
            threshold: 50,
            target: 35,
            hard_limit,
        };
        h
    }

    macro_rules! assert_handled {
        ($h:expr, $now:expr, $event:expr, @$snapshot:literal) => {{
            let (result, step) = $h.step($now, $event).await;
            result.unwrap();
            insta::assert_yaml_snapshot!(step, @$snapshot);
        }};
    }

    macro_rules! assert_rejected {
        ($h:expr, $now:expr, $event:expr, @$snapshot:literal) => {{
            let (result, step) = $h.step($now, $event).await;
            insta::assert_yaml_snapshot!((result.unwrap_err().to_string(), step), @$snapshot);
        }};
    }

    #[tokio::test]
    async fn submit_starts_turn() {
        let mut h = Harness::with(&[]).await;
        assert_handled!(h, 7, submit("hi", 0), @"
        busy: true
        ui:
          - HistoryUpdate:
              - 0
              - GenerationIncremented
          - HistoryUpdate:
              - 1
              - UserMessage:
                  text: hi
                  token_count: 1
                  created_at: 7
          - HistoryUpdate:
              - 1
              - TurnResponse:
                  Created:
                    created_at: 7
        tasks:
          - - 0
            - turn
        ");
    }

    #[tokio::test]
    async fn submit_with_stale_generation_is_rejected() {
        let mut h = Harness::with(&[]).await;
        assert_rejected!(h, 7, submit("hi", 1), @r#"
        - "history generation mismatch: expected 0"
        - busy: false
          ui: []
          tasks: []
        "#);
    }

    #[tokio::test]
    async fn submit_while_busy_queues_as_pending() {
        let mut h = Harness::with(&[]).await;
        h.ledger.register(Task::turn());
        assert_handled!(h, 7, submit("hi", 0), @"
        busy: true
        ui: []
        tasks: []
        ");
        assert_eq!(h.state.pending_messages.len(), 1);
    }

    /// a mid-turn submit — even one stamped with a nonsense generation —
    /// is queued and flushes into its own turn at turn end, so typed input
    /// is never destroyed by a busy rejection
    #[tokio::test]
    async fn busy_submit_flushes_into_a_turn_at_turn_end() {
        let mut h = Harness::with(&[]).await;
        let turn = h.step(1, submit("hi", 0)).await.1.task();
        h.step(2, submit("steer", 999)).await.0.unwrap();
        assert_eq!(h.state.pending_messages.len(), 1);

        h.step(
            3,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 3 }),
        )
        .await
        .0
        .unwrap();
        let (result, step) = h
            .step(4, AgentEvent::Done(turn, Ok(TaskOutput::Turn)))
            .await;
        result.unwrap();
        assert!(serde_json::to_string(&step.ui).unwrap().contains("steer"));
        assert!(step.starts_turn());
        assert!(h.state.pending_messages.is_empty());
    }

    /// finding 3: a stale assistant id (e.g. removed from config after a
    /// restore) fails the submit cleanly — no queued turn message, no
    /// dangling ledger task — and the idle agent accepts the fix
    #[tokio::test]
    async fn submit_with_unknown_assistant_fails_without_wedging() {
        let mut h = Harness::with(&[]).await;
        h.state.assistant_id = "gone".into();
        let (result, step) = h.step(7, submit("hi", 0)).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "unknown assistant \"gone\"".to_string()
        );
        assert!(!step.starts_turn(), "{:?}", step.tasks);
        h.idle().unwrap();
        // a resubmit after the fix turns normally
        h.step(8, user(UserCommand::SetAssistant("test".into())))
            .await
            .0
            .unwrap();
        h.step(9, submit("retry", 1)).await.0.unwrap();
        assert!(h.busy());
    }

    #[tokio::test]
    async fn abort_fails_inflight_turn() {
        let mut h = Harness::with(&[]).await;
        h.step(7, submit("hi", 0)).await.0.unwrap();
        assert_handled!(h, 9, user(UserCommand::Abort), @"
        busy: false
        ui:
          - HistoryUpdate:
              - 1
              - GenerationIncremented
          - HistoryUpdate:
              - 2
              - TurnResponse:
                  Failed:
                    message: aborted by user
                    ended_at: 9
        tasks: []
        ");
        assert!(matches!(
            h.history().state().last(),
            Some(Message::Assistant(crate::llm::history::message::AssistantMessage {
                status: crate::llm::history::message::AssistantStatus::Error(msg),
                ..
            })) if msg == ABORTED_BY_USER
        ));
    }

    #[tokio::test]
    async fn task_failure_emits_error_and_fails_turn() {
        let mut h = Harness::with(&[]).await;
        let history = h.history_mut();
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
        let tid = h.ledger.register(Task::turn());

        assert_handled!(h, 7, AgentEvent::Done(tid, Err("oops".into())), @"
        busy: false
        ui:
          - Error: oops
          - HistoryUpdate:
              - 0
              - TurnResponse:
                  Failed:
                    message: oops
                    ended_at: 7
        tasks: []
        ");
    }

    #[tokio::test]
    async fn set_assistant_rejected_while_busy() {
        let mut h = Harness::with(&[]).await;
        h.ledger.register(Task::turn());
        assert_rejected!(h, 7, user(UserCommand::SetAssistant("test2".into())), @"
        - agent is busy
        - busy: true
          ui: []
          tasks: []
        ");
        assert_eq!(h.state.assistant_id, "test");
    }

    #[tokio::test]
    async fn set_assistant_switches_saves_and_emits() {
        let mut h = Harness::with(&[]).await;
        assert_handled!(h, 7, user(UserCommand::SetAssistant("test2".into())), @"
        busy: false
        ui:
          - AssistantSet: test2
        tasks: []
        ");
        assert_eq!(h.state.assistant_id, "test2");
        let saved = h.project.store().load_state(&h.id).await.unwrap();
        assert_eq!(saved.assistant_id, "test2");
    }

    #[tokio::test]
    async fn set_assistant_unknown_id_is_rejected() {
        let mut h = Harness::with(&[]).await;
        assert_rejected!(h, 7, user(UserCommand::SetAssistant("nope".into())), @r#"
        - "unknown assistant \"nope\""
        - busy: false
          ui: []
          tasks: []
        "#);
    }

    /// a rejected duplicate (busy original) reports back naming the copy,
    /// so the app can roll its preview tab back
    #[tokio::test]
    async fn duplicate_while_busy_reports_failure_for_the_copy() {
        let mut h = Harness::with(&[]).await;
        h.ledger.register(Task::turn());
        let copy = AgentId::from("copy".to_string());
        assert_handled!(h, 7, user(UserCommand::Duplicate(copy)), @"
        busy: true
        ui:
          - DuplicateFailed:
              copy: copy
              error: agent is busy
        tasks: []
        ");
    }

    #[tokio::test]
    async fn abort_while_idle_emits_no_history_event() {
        let mut h = Harness::with(&[]).await;
        assert_handled!(h, 9, user(UserCommand::Abort), @"
        busy: false
        ui:
          - HistoryUpdate:
              - 0
              - GenerationIncremented
        tasks: []
        ");
    }

    #[tokio::test]
    async fn task_failure_after_abort_surfaces_error_without_reply() {
        let mut h = Harness::with(&[]).await;
        let tid = h.step(1, submit("hi", 0)).await.1.task();
        h.step(2, user(UserCommand::Abort)).await.0.unwrap();

        assert_handled!(h, 3, AgentEvent::Done(tid, Err("stream closed".into())), @"
        busy: false
        ui:
          - Error: stream closed
        tasks: []
        ");
    }

    #[tokio::test]
    async fn task_event_after_abort_is_dropped() {
        let mut h = Harness::with(&[]).await;
        let tid = h.step(1, submit("hi", 0)).await.1.task();
        h.step(2, user(UserCommand::Abort)).await.0.unwrap();

        assert_handled!(h, 3, AgentEvent::Stream(tid, text_output("out", "late")), @"
        busy: false
        ui: []
        tasks: []
        ");
    }

    #[tokio::test]
    async fn task_done_starts_followup_turn_after_tool_resolves() {
        let mut h = Harness::with(&[]).await;
        let turn = h.step(1, submit("hi", 0)).await.1.task();

        let (result, step) = h.step(2, AgentEvent::Stream(turn, todo_call(None))).await;
        result.unwrap();
        let tool = step.task();
        insta::assert_yaml_snapshot!(step, @r#"
        busy: true
        ui:
          - HistoryUpdate:
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
        tasks:
          - - 1
            - tool
        "#);

        h.step(
            3,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 3 }),
        )
        .await
        .0
        .unwrap();
        // the turn task finishing leaves the tool task pending: no new turn yet
        assert_handled!(h, 4, AgentEvent::Done(turn, Ok(TaskOutput::Turn)), @"
        busy: true
        ui: []
        tasks: []
        ");

        // the reaper's terminal resolves the slot and starts the follow-up
        assert_handled!(
            h, 6,
            AgentEvent::Done(tool, Ok(TaskOutput::Tool(Box::new(todo_item(Some(Ok(TodoResult {}))))))),
            @r#"
        busy: true
        ui:
          - HistoryUpdate:
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
          - HistoryUpdate:
              - 1
              - TurnResponse:
                  Created:
                    created_at: 6
        tasks:
          - - 2
            - turn
        "#
        );
    }

    #[tokio::test]
    async fn reaped_tool_failure_patches_slot_and_starts_followup() {
        let mut h = Harness::with(&[]).await;
        let turn = h.step(1, submit("hi", 0)).await.1.task();
        let tool = h
            .step(2, AgentEvent::Stream(turn, todo_call(None)))
            .await
            .1
            .task();
        h.step(
            3,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 3 }),
        )
        .await
        .0
        .unwrap();
        h.step(4, AgentEvent::Done(turn, Ok(TaskOutput::Turn)))
            .await
            .0
            .unwrap();

        h.step(5, AgentEvent::Output(tool, "abc".into()))
            .await
            .0
            .unwrap();
        // a panic finalizes: slot gets the error plus the streamed partial,
        // ledger unsticks, and the model sees the failure next turn
        assert_handled!(
            h, 5,
            AgentEvent::Done(tool, Err("tool panicked: boom".into())),
            @r#"
        busy: true
        ui:
          - HistoryUpdate:
              - 1
              - ToolCallFailed:
                  call_id: call-1
                  error: "tool panicked: boom; partial output:\nabc"
          - HistoryUpdate:
              - 1
              - TurnResponse:
                  Created:
                    created_at: 5
        tasks:
          - - 2
            - turn
        "#
        );
        // the slot itself holds the error (the follow-up Created appended a
        // fresh assistant message after it)
        assert!(
            h.history()
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

    #[tokio::test]
    async fn message_while_idle_appends_and_wakes() {
        let mut h = Harness::with(&[]).await;
        assert_handled!(h, 7, message("[from: kid]\nhi"), @r#"
        busy: true
        ui:
          - HistoryUpdate:
              - 0
              - UserMessage:
                  text: "[from: kid]\nhi"
                  token_count: 5
                  created_at: 5
          - HistoryUpdate:
              - 0
              - TurnResponse:
                  Created:
                    created_at: 7
        tasks:
          - - 0
            - turn
        "#);
        assert!(h.state.pending_messages.is_empty());
    }

    #[tokio::test]
    async fn message_while_busy_buffers_then_flushes_on_idle() {
        let mut h = Harness::with(&[]).await;
        let turn = h.step(1, submit("hi", 0)).await.1.task();

        // turn still streaming: buffered, save only
        assert_handled!(h, 2, message("[from: kid]\nearly bird"), @"
        busy: true
        ui: []
        tasks: []
        ");
        assert_eq!(h.state.pending_messages.len(), 1);

        h.step(
            3,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 3 }),
        )
        .await
        .0
        .unwrap();
        // idle: the buffer flushes and wakes the agent
        assert_handled!(h, 4, AgentEvent::Done(turn, Ok(TaskOutput::Turn)), @r#"
        busy: true
        ui:
          - HistoryUpdate:
              - 1
              - UserMessage:
                  text: "[from: kid]\nearly bird"
                  token_count: 6
                  created_at: 5
          - HistoryUpdate:
              - 1
              - TurnResponse:
                  Created:
                    created_at: 4
        tasks:
          - - 1
            - turn
        "#);
        assert!(h.state.pending_messages.is_empty());
    }

    /// a message that can't start its turn still lands in history and is
    /// saved; the error goes to the caller, and the agent stays idle
    #[tokio::test]
    async fn failed_wake_keeps_the_message() {
        let mut h = Harness::with(&[]).await;
        h.state.assistant_id = "gone".into();
        assert_rejected!(h, 5, message("[from: kid]\nhi"), @r#"
        - "unknown assistant \"gone\""
        - busy: false
          ui:
            - HistoryUpdate:
                - 0
                - UserMessage:
                    text: "[from: kid]\nhi"
                    token_count: 5
                    created_at: 5
          tasks: []
        "#);
        let saved = h.project.store().load_state(&h.id).await.unwrap();
        assert!(format!("{:?}", saved.context.history).contains("[from: kid]"));
    }

    #[tokio::test]
    async fn stale_submit_is_rejected_before_flush() {
        let mut h = Harness::with(&[]).await;
        h.step(1, submit("hi", 0)).await.0.unwrap();
        h.step(2, message("[from: kid]\nstranded")).await.0.unwrap();
        // abort clears the ledger but deliberately never flushes
        h.step(3, user(UserCommand::Abort)).await.0.unwrap();
        assert_eq!(h.state.pending_messages.len(), 1);

        // stale generation: rejected before the flush — the buffer is intact
        let (result, step) = h.step(4, submit("late", 1)).await;
        assert!(result.is_err());
        assert!(step.ui.is_empty() && step.tasks.is_empty());
        assert_eq!(h.state.pending_messages.len(), 1);
    }

    #[tokio::test]
    async fn submit_flushes_stranded_buffer_before_user_message() {
        let mut h = Harness::with(&[]).await;
        h.step(1, submit("hi", 0)).await.0.unwrap();
        h.step(2, message("[from: kid]\nstranded")).await.0.unwrap();
        h.step(3, user(UserCommand::Abort)).await.0.unwrap();

        let (result, step) = h.step(4, submit("continue", 2)).await;
        result.unwrap();
        let positions: Vec<usize> = ["stranded", "continue"]
            .iter()
            .map(|needle| {
                step.ui
                    .iter()
                    .position(|e| serde_json::to_string(e).unwrap().contains(*needle))
                    .unwrap_or_else(|| panic!("{needle} not delivered"))
            })
            .collect();
        assert!(
            positions[0] < positions[1],
            "pending message must precede the user message"
        );
        assert!(h.state.pending_messages.is_empty());
    }

    #[tokio::test]
    async fn resume_flushes_buffered_messages_and_starts_turn() {
        let mut h = Harness::with(&[]).await;
        h.step(1, submit("hi", 0)).await.0.unwrap();
        h.step(2, message("[from: kid]\npersisted"))
            .await
            .0
            .unwrap();
        h.step(3, user(UserCommand::Abort)).await.0.unwrap();

        // simulated restart: rebuild the agent from the persisted state
        let mut restored = h.restart().await;
        assert_eq!(restored.state.pending_messages.len(), 1);

        // resume (the startup wake) delivers the buffered message and starts
        // its turn — no submit or other poke needed
        restored.resume(5).await.unwrap();
        let step = restored.drain();
        assert!(restored.state.pending_messages.is_empty());
        assert!(
            serde_json::to_string(&step.ui)
                .unwrap()
                .contains("persisted")
        );
        assert!(step.starts_turn());
    }

    #[tokio::test]
    async fn buffered_message_survives_restore_and_flushes_on_submit() {
        let mut h = Harness::with(&[]).await;
        h.step(1, submit("hi", 0)).await.0.unwrap();
        h.step(2, message("[from: kid]\npersisted"))
            .await
            .0
            .unwrap();
        h.step(3, user(UserCommand::Abort)).await.0.unwrap();

        let mut restored = h.restart().await;
        assert_eq!(restored.state.pending_messages.len(), 1);

        // restored generation resets to 0: it is #[serde(skip)]
        let (result, step) = restored.step(5, submit("go", 0)).await;
        result.unwrap();
        assert!(
            serde_json::to_string(&step.ui)
                .unwrap()
                .contains("persisted")
        );
        assert!(restored.state.pending_messages.is_empty());
    }

    #[tokio::test]
    async fn task_done_returns_to_idle() {
        let mut h = Harness::with(&[]).await;
        let tid = h.step(1, submit("hi", 0)).await.1.task();
        h.step(2, AgentEvent::Stream(tid, text_output("out", "done")))
            .await
            .0
            .unwrap();
        h.step(
            3,
            AgentEvent::Stream(tid, AssistantEvent::Completed { ended_at: 3 }),
        )
        .await
        .0
        .unwrap();

        assert_handled!(h, 4, AgentEvent::Done(tid, Ok(TaskOutput::Turn)), @"
        busy: false
        ui: []
        tasks: []
        ");
    }

    #[tokio::test]
    async fn task_done_failure_surfaces_error_and_fails_turn() {
        let mut h = Harness::with(&[]).await;
        let tid = h.step(1, submit("hi", 0)).await.1.task();
        h.step(
            2,
            AgentEvent::Stream(
                tid,
                AssistantEvent::Failed {
                    message: "boom".into(),
                    ended_at: 2,
                },
            ),
        )
        .await
        .0
        .unwrap();

        assert_handled!(h, 3, AgentEvent::Done(tid, Err("boom".into())), @"
        busy: false
        ui:
          - Error: boom
          - HistoryUpdate:
              - 1
              - TurnResponse:
                  Failed:
                    message: boom
                    ended_at: 3
        tasks: []
        ");
    }

    #[tokio::test]
    async fn resolved_tool_call_is_not_rerun() {
        let mut h = Harness::with(&[]).await;
        let tid = h.step(1, submit("hi", 0)).await.1.task();

        assert_handled!(
            h, 2,
            AgentEvent::Stream(tid, todo_call(Some(Ok(TodoResult {})))),
            @r#"
        busy: true
        ui:
          - HistoryUpdate:
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
        tasks: []
        "#
        );
    }

    #[tokio::test]
    async fn retry_failed_turn_starts_normal_turn() {
        let mut h = Harness::with(&[]).await;
        let history = h.history_mut();
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

        assert_handled!(h, 7, user(UserCommand::Retry), @"
        busy: true
        ui:
          - HistoryUpdate:
              - 0
              - GenerationIncremented
          - HistoryUpdate:
              - 1
              - TurnResponse:
                  Created:
                    created_at: 7
        tasks:
          - - 0
            - turn
        ");
    }

    #[tokio::test]
    async fn undo_pops_messages() {
        let mut h = Harness::with(&["first", "second"]).await;
        assert_handled!(h, 7, user(UserCommand::Undo(1)), @"
        busy: false
        ui:
          - HistoryUpdate:
              - 0
              - GenerationIncremented
          - HistoryUpdate:
              - 1
              - Pop: 1
        tasks: []
        ");
        assert_eq!(h.history().state().messages.len(), 1);
    }

    #[tokio::test]
    async fn compact_runs_alongside_turn_and_applies_at_turn_end() {
        let mut h = Harness::with(&["first"]).await;
        let turn = h.step(1, submit("hi", 0)).await.1.task();

        // the in-flight assistant message is left out of the summary
        let (result, step) = h.step(2, user(UserCommand::Compact(99))).await;
        result.unwrap();
        let compact = step.summary();
        insta::assert_yaml_snapshot!(step, @"
        busy: true
        ui: []
        tasks:
          - - 1
            - summary
        ");
        assert!(matches!(
            h.ledger.get(compact),
            Some(Task::Compact { n_drop: 2 })
        ));

        // the summary lands mid-turn: held until the turn ends
        assert_handled!(h, 3, AgentEvent::Done(compact, summary("gist")), @"
        busy: true
        ui: []
        tasks: []
        ");
        h.step(4, AgentEvent::Stream(turn, text_output("out", "done")))
            .await
            .0
            .unwrap();
        h.step(
            5,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 5 }),
        )
        .await
        .0
        .unwrap();
        assert_handled!(h, 6, AgentEvent::Done(turn, Ok(TaskOutput::Turn)), @"
        busy: false
        ui:
          - HistoryUpdate:
              - 1
              - Compact:
                  n_drop: 2
                  summary:
                    text: gist
                    token_count: 0
                    created_at: 1
                    started_at: 2
                    ended_at: 3
        tasks: []
        ");
        insta::assert_yaml_snapshot!(h.history().state().messages, @"
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

    #[tokio::test]
    async fn compact_while_idle_applies_on_arrival() {
        let mut h = Harness::with(&["first", "second"]).await;
        let compact = h.step(1, user(UserCommand::Compact(1))).await.1.summary();

        assert_handled!(h, 2, AgentEvent::Done(compact, summary("gist")), @"
        busy: false
        ui:
          - HistoryUpdate:
              - 0
              - Compact:
                  n_drop: 1
                  summary:
                    text: gist
                    token_count: 0
                    created_at: 1
                    started_at: 2
                    ended_at: 3
        tasks: []
        ");
        assert_eq!(h.history().state().messages.len(), 2);
    }

    #[tokio::test]
    async fn submit_while_compacting_starts_turn_right_away() {
        let mut h = Harness::with(&["first"]).await;
        h.step(1, user(UserCommand::Compact(1))).await.0.unwrap();

        let (result, step) = h.step(2, submit("hi", 0)).await;
        result.unwrap();
        assert!(step.starts_turn());
        assert!(h.ledger.in_turn() && h.ledger.compacting());
    }

    #[tokio::test]
    async fn commands_rejected_while_compacting() {
        let mut h = Harness::with(&["first", "second"]).await;
        h.step(1, user(UserCommand::Compact(1))).await.0.unwrap();

        assert_rejected!(h, 2, user(UserCommand::Compact(1)), @"
        - already compacting
        - busy: true
          ui: []
          tasks: []
        ");
        assert_rejected!(h, 3, user(UserCommand::Undo(1)), @"
        - agent is busy
        - busy: true
          ui: []
          tasks: []
        ");
    }

    #[tokio::test]
    async fn compact_zero_messages_is_rejected() {
        let mut h = Harness::with(&["short"]).await;
        assert_rejected!(h, 7, user(UserCommand::Compact(0)), @"
        - nothing to compact
        - busy: false
          ui: []
          tasks: []
        ");
        assert!(h.ledger.idle());
    }

    #[tokio::test]
    async fn abort_drops_compaction() {
        let mut h = Harness::with(&["first"]).await;
        h.step(1, submit("hi", 0)).await.0.unwrap();
        let compact = h.step(2, user(UserCommand::Compact(1))).await.1.summary();

        h.step(3, user(UserCommand::Abort)).await.0.unwrap();
        assert!(!h.compacting());

        // the late summary is stale
        assert_handled!(h, 4, AgentEvent::Done(compact, summary("gist")), @"
        busy: false
        ui: []
        tasks: []
        ");
        assert!(h.compaction.is_none());
    }

    /// abort strands the buffer on purpose: a summary landing later must
    /// not flush it into a turn the user didn't ask for
    #[tokio::test]
    async fn summary_after_abort_keeps_stranded_buffer() {
        let mut h = Harness::with(&["first"]).await;
        h.step(1, submit("hi", 0)).await.0.unwrap();
        h.step(2, message("[from: kid]\nstranded")).await.0.unwrap();
        h.step(3, user(UserCommand::Abort)).await.0.unwrap();
        let compact = h.step(4, user(UserCommand::Compact(1))).await.1.summary();

        let (result, step) = h.step(5, AgentEvent::Done(compact, summary("gist"))).await;
        result.unwrap();

        assert!(!step.starts_turn());
        assert_eq!(h.state.pending_messages.len(), 1);
        assert!(h.ledger.idle());
    }

    #[tokio::test]
    async fn autocompact_past_threshold_runs_alongside_the_turn() {
        let mut h = near_full(100).await;

        let (result, step) = h.step(1, submit("hi", 0)).await;
        result.unwrap();

        let compact = step.summary();
        assert!(matches!(
            h.ledger.get(compact),
            Some(Task::Compact { n_drop: 1 })
        ));
        assert!(step.starts_turn());
    }

    #[tokio::test]
    async fn autocompact_below_threshold_does_nothing() {
        let mut h = near_full(100).await;
        h.project.config_mut().compact.threshold = 90;

        let (result, step) = h.step(1, submit("hi", 0)).await;
        result.unwrap();

        assert!(!h.ledger.compacting());
        assert!(step.starts_turn());
    }

    #[tokio::test]
    async fn hard_limit_holds_turn_until_summary_lands() {
        let mut h = near_full(50).await;

        let (result, step) = h.step(1, submit("hi", 0)).await;
        result.unwrap();
        let compact = step.summary();
        assert!(!step.starts_turn());
        // the prompt is in history, its turn is held
        assert!(matches!(
            h.history().state().last(),
            Some(Message::User(UserMessage { text, .. })) if text == "hi"
        ));
        assert!(h.busy() && h.compacting());

        // held = busy: a second prompt lands without starting a turn either
        let (result, step) = h.step(2, submit("more", 999)).await;
        result.unwrap();
        assert!(!step.starts_turn());

        let (result, step) = h.step(3, AgentEvent::Done(compact, summary("gist"))).await;
        result.unwrap();
        assert!(step.starts_turn());
        assert!(!h.compacting());
        insta::assert_yaml_snapshot!(
            h.history().state().messages,
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

    #[tokio::test]
    async fn failed_summary_stops_held_turn() {
        let mut h = near_full(50).await;
        let compact = h.step(1, submit("hi", 0)).await.1.summary();

        assert_handled!(h, 2, AgentEvent::Done(compact, Err("rate limited".into())), @"
        busy: false
        ui:
          - Error: rate limited
        tasks: []
        ");

        // retry asks for a fresh summary, and holds the turn again
        let (result, step) = h.step(3, user(UserCommand::Retry)).await;
        result.unwrap();
        step.summary();
        assert!(!step.starts_turn());
    }
}

use anyhow::Result;
use tracing::debug;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::PendingMessage;
use crate::agent::event::AgentEvent;
use crate::agent::event::TaskOutput;
use crate::agent::event::TaskResult;
use crate::agent::event::UiEvent;
use crate::agent::event::UserCommand;
use crate::agent::task::Task;
use crate::agent::task::TaskId;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::agent::tool::registry::TOOL_REGISTRY;
use crate::llm::history::AssistantEvent;
use crate::llm::history::Compaction;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::ToolCallItem;
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
            AgentEvent::User(command) => self.handle_command(now, command).await,
            AgentEvent::Message(msg) => self.deliver(now, msg.into()),
            AgentEvent::Stream(tid, event) => match self.tasks.get(tid) {
                Some(Task::Turn) => self.handle_turn_event(event),
                Some(task) => unreachable!("stream event from {task:?}"),
                None => Ok(()),
            },
            AgentEvent::Output(tid, chunk) => self.handle_tool_output(tid, chunk),
            AgentEvent::Done(tid, result) => self.handle_task_done(now, tid, result),
        };
        result.and(self.save().await)
    }

    async fn save(&mut self) -> Result<()> {
        if !std::mem::take(&mut self.dirty) {
            return Ok(());
        }
        self.state.save(&self.project, &self.id).await
    }

    async fn handle_command(
        &mut self,
        now: u64,
        command: UserCommand,
    ) -> Result<()> {
        match command {
            // queued while busy, so it doubles as steering; flushes after
            // any messages stranded by abort -- abort itself never flushes:
            // a naive flush would auto-start a turn on user abort
            UserCommand::Submit(text) => self.deliver(now, UserMessage::new(text, now).into()),
            UserCommand::Compact(n) => self.compact(now, n),
            UserCommand::Retry => {
                self.ensure_idle()?;
                self.needs_turn = true;
                self.advance(now)
            }
            UserCommand::Abort => self.abort(now),
            UserCommand::Undo(n) => {
                self.ensure_idle()?;
                self.handle_history_update(HistoryUpdate::Pop(n))
            }
            UserCommand::SetAssistant(id) => {
                self.ensure_idle()?;
                let id = self.project.assistants().assistant(&id)?.id;
                self.emit(UiEvent::AssistantSet(id.clone()));
                self.state.assistant_id = id;
                self.dirty = true;
                Ok(())
            }
            UserCommand::Duplicate(copy) => {
                if let Err(e) = self.try_duplicate(copy.clone()).await {
                    let error = format!("{e:#}");
                    self.emit(UiEvent::DuplicateFailed { copy, error });
                }
                Ok(())
            }
        }
    }

    pub fn busy(&self) -> bool {
        !self.tasks.idle()
    }

    pub fn ensure_idle(&self) -> Result<()> {
        anyhow::ensure!(!self.busy(), "agent is busy");
        Ok(())
    }

    /// a summary is coming, or ready to apply
    fn compacting(&self) -> bool {
        self.tasks.compacting() || matches!(self.compaction, Some(Ok(_)))
    }

    fn handle_history_update(
        &mut self,
        event: HistoryUpdate,
    ) -> Result<()> {
        self.history_mut().handle(event.clone())?;
        self.dirty |= !matches!(
            event,
            HistoryUpdate::TurnResponse(AssistantEvent::Delta(_))
                | HistoryUpdate::ToolOutput { .. }
        );
        self.emit(UiEvent::HistoryUpdate(event));
        Ok(())
    }

    fn handle_turn_event(
        &mut self,
        event: AssistantEvent,
    ) -> Result<()> {
        let call = match &event {
            AssistantEvent::Item(item) => match &**item {
                AssistantItem::ToolCall(call) => Some(call.clone()),
                _ => None,
            },
            _ => None,
        };
        self.handle_history_update(HistoryUpdate::TurnResponse(event))?;
        if let Some(call) = call {
            self.run_tool_call(call);
        }
        Ok(())
    }

    fn run_tool_call(
        &mut self,
        mut call: ToolCallItem,
    ) {
        let inherited_history = call
            .task
            .inherit_history()
            .then(|| self.history().subagent());
        let task = Task::Tool {
            call_id: call.call_id.clone(),
        };
        self.tasks.spawn(task, |sink| {
            let ctx = ToolRuntimeContext::new(
                self.id.clone(),
                self.project.clone(),
                self.router.clone(),
                sink,
                inherited_history,
            );
            async move {
                call.task.run(ctx).await;
                call.touch_ready_at_now();
                Ok(TaskOutput::Tool(Box::new(call)))
            }
        });
    }

    fn handle_tool_output(
        &mut self,
        tid: TaskId,
        chunk: String,
    ) -> Result<()> {
        match self.tasks.get(tid) {
            Some(Task::Tool { call_id }) => {
                let call_id = call_id.clone();
                self.handle_history_update(HistoryUpdate::ToolOutput { call_id, chunk })
            }
            Some(task) => unreachable!("appending tool output chunk to a non-tool task: {task:?}"),
            None => Ok(()),
        }
    }

    fn handle_task_done(
        &mut self,
        now: u64,
        tid: TaskId,
        result: TaskResult,
    ) -> Result<()> {
        let Some(task) = self.tasks.finish(tid) else {
            return Ok(());
        };
        match (task, result) {
            (Task::Turn, Ok(_)) => {}
            (Task::Turn, Err(message)) => {
                tracing::error!("turn error in agent {}: {message}", self.id);
                self.emit(UiEvent::Error(message.clone()));
                let failed = AssistantEvent::Failed {
                    message,
                    ended_at: now,
                };
                self.handle_history_update(HistoryUpdate::TurnResponse(failed))?;
            }
            (Task::Tool { .. }, Ok(TaskOutput::Tool(item))) => {
                let item = AssistantEvent::Item(Box::new(AssistantItem::ToolCall(*item)));
                self.handle_history_update(HistoryUpdate::TurnResponse(item))?;
            }
            (Task::Tool { call_id }, result) => {
                let error = result
                    .err()
                    .unwrap_or_else(|| "tool returned no item".into());
                self.handle_history_update(HistoryUpdate::ToolCallFailed { call_id, error })?;
            }
            (Task::Compact { n_drop }, Ok(TaskOutput::Summary(summary))) => {
                self.compaction = Some(Ok(Compaction { n_drop, summary }));
                return self.advance(now);
            }
            (Task::Compact { .. }, result) => {
                let error = result
                    .err()
                    .unwrap_or_else(|| "compaction returned no summary".into());
                self.emit(UiEvent::Error(error.clone()));
                self.compaction = Some(Err(error));
                return self.advance(now);
            }
        }
        self.needs_turn |= self.history().state().ends_with_tool_calls();
        self.advance(now)
    }

    /// turn boundary, a no-op while a turn is in flight
    fn advance(
        &mut self,
        now: u64,
    ) -> Result<()> {
        if self.tasks.in_turn() {
            return Ok(());
        }
        if let Some(Ok(compaction)) = self.compaction.take_if(|c| c.is_ok()) {
            self.handle_history_update(HistoryUpdate::Compact(compaction))?;
        }
        if !self.needs_turn {
            return Ok(());
        }
        self.flush_pending()?;
        self.autocompact(now)?;
        if self.past(self.project.config().compact.hard_limit) {
            return match &self.compaction {
                _ if self.compacting() => Ok(()),
                Some(Err(e)) => Err(anyhow::anyhow!(
                    "past the hard limit, compaction failed: {e}"
                )),
                _ => Err(anyhow::anyhow!("past the hard limit, nothing to compact")),
            };
        }
        self.needs_turn = false;
        self.start_turn(now)
    }

    fn deliver(
        &mut self,
        now: u64,
        msg: PendingMessage,
    ) -> Result<()> {
        self.state.pending_messages.push(msg);
        self.dirty = true;
        self.needs_turn = true;
        self.advance(now)
    }

    fn flush_pending(&mut self) -> Result<()> {
        for msg in std::mem::take(&mut self.state.pending_messages) {
            self.handle_history_update(msg.into())?;
        }
        Ok(())
    }

    fn abort(
        &mut self,
        now: u64,
    ) -> Result<()> {
        let in_turn = self.tasks.in_turn();
        self.tasks.abort_all();
        self.compaction.take_if(|c| c.is_ok());
        self.needs_turn = false;
        if !in_turn {
            return Ok(());
        }
        self.handle_history_update(HistoryUpdate::FailUnresolved {
            message: ABORTED_BY_USER.into(),
            ended_at: now,
        })
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
        // a failure in `compaction` turns autocompact off
        if self.tasks.compacting() || self.compaction.is_some() || !self.past(config.threshold) {
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
        let stable = if self.tasks.in_turn() { len - 1 } else { len };
        let n_drop = n_drop.min(stable);
        anyhow::ensure!(n_drop > 0, "nothing to compact");
        let assistant = self
            .project
            .assistants()
            .assistant(&self.state.assistant_id)?;
        let instructions = self.history().instructions().to_string();
        let messages = self.history().compact_input(n_drop, now);
        // clears a failure, turning autocompact back on
        self.compaction = None;
        self.tasks.spawn(Task::Compact { n_drop }, |_| async move {
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
        // resolve the fallible lookup before any history/task mutation: a
        // stale assistant id must fail the submit, not wedge the agent busy
        let assistant = self
            .project
            .assistants()
            .assistant(&self.state.assistant_id)?;
        // clone before the Created event appends the queued assistant message
        let messages = self.history().state().messages.clone();
        let instructions = self.history().instructions().to_string();
        let created = AssistantEvent::Created { created_at: now };
        self.handle_history_update(HistoryUpdate::TurnResponse(created))?;
        self.tasks.spawn(Task::Turn, |sink| async move {
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

#[cfg(test)]
mod tests {
    use derive_more::Deref;
    use derive_more::DerefMut;
    use similar_asserts::assert_eq;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::agent::AgentId;
    use crate::agent::task::Tasks;
    use crate::config::CompactConfig;
    use crate::llm::history::message::CompactMessage;
    use crate::llm::history::message::Message;
    use crate::llm::history::message::OutputContent;
    use crate::llm::history::message::OutputItem;
    use crate::llm::history::message::PeerMessage;
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
                h.history_mut().handle(user_message(text)).unwrap();
            }
            h
        }

        fn wrap(
            mut agent: Agent,
            app_rx: UnboundedReceiver<AppEvent>,
        ) -> Self {
            agent.tasks = Tasks::held();
            Self { agent, app_rx }
        }

        /// a fresh agent on the persisted state, as after a restart
        async fn restart(&self) -> Self {
            let state = self.project.store().load_state(&self.id).await.unwrap();
            let (app_tx, app_rx) = unbounded_channel();
            let mut agent = Agent::new(self.router.clone(), self.id.clone(), state);
            agent.app_tx = app_tx;
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
            let tasks = self.agent.tasks.take_held();
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

    fn submit(text: &str) -> AgentEvent {
        user(UserCommand::Submit(text.into()))
    }

    fn message(text: &str) -> AgentEvent {
        AgentEvent::Message(PeerMessage::new("kid", text, 5))
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
            streamed: String::new(),
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

    /// the first tool call's output
    fn call_output(agent: &Agent) -> Option<String> {
        agent
            .history()
            .state()
            .iter()
            .filter_map(|m| m.try_as_assistant_ref())
            .flat_map(|m| m.content.values())
            .find_map(|item| match item {
                AssistantItem::ToolCall(call) => call.task.output(),
                _ => None,
            })
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
        assert_handled!(h, 7, submit("hi"), @"
        busy: true
        ui:
          - HistoryUpdate:
              UserMessage:
                text: hi
                token_count: 1
                created_at: 7
          - HistoryUpdate:
              TurnResponse:
                Created:
                  created_at: 7
        tasks:
          - - 0
            - turn
        ");
    }

    #[tokio::test]
    async fn submit_while_busy_queues_as_pending() {
        let mut h = Harness::with(&[]).await;
        h.tasks.register(Task::Turn);
        assert_handled!(h, 7, submit("hi"), @"
        busy: true
        ui: []
        tasks: []
        ");
        assert_eq!(h.state.pending_messages.len(), 1);
    }

    /// a mid-turn submit is queued and flushes into its own turn at turn
    /// end, so typed input is never destroyed by a busy rejection
    #[tokio::test]
    async fn busy_submit_flushes_into_a_turn_at_turn_end() {
        let mut h = Harness::with(&[]).await;
        let turn = h.step(1, submit("hi")).await.1.task();
        h.step(2, submit("steer")).await.0.unwrap();
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
    /// dangling task — and the idle agent accepts the fix
    #[tokio::test]
    async fn submit_with_unknown_assistant_fails_without_wedging() {
        let mut h = Harness::with(&[]).await;
        h.state.assistant_id = "gone".into();
        let (result, step) = h.step(7, submit("hi")).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "unknown assistant \"gone\"".to_string()
        );
        assert!(!step.starts_turn(), "{:?}", step.tasks);
        h.ensure_idle().unwrap();
        // a resubmit after the fix turns normally
        h.step(8, user(UserCommand::SetAssistant("test".into())))
            .await
            .0
            .unwrap();
        h.step(9, submit("retry")).await.0.unwrap();
        assert!(h.busy());
    }

    #[tokio::test]
    async fn abort_fails_inflight_turn() {
        let mut h = Harness::with(&[]).await;
        h.step(7, submit("hi")).await.0.unwrap();
        assert_handled!(h, 9, user(UserCommand::Abort), @"
        busy: false
        ui:
          - HistoryUpdate:
              FailUnresolved:
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

    /// one channel: output sent before an abort is handled before it, so the
    /// aborted call keeps it; output sent after it is stale
    #[tokio::test]
    async fn abort_keeps_output_sent_before_it() {
        let mut h = Harness::with(&[]).await;
        let turn = h.step(1, submit("hi")).await.1.task();
        let tool = h
            .step(2, AgentEvent::Stream(turn, todo_call(None)))
            .await
            .1
            .task();
        for event in [
            AgentEvent::Output(tool, "early".into()),
            user(UserCommand::Abort),
            AgentEvent::Output(tool, "late".into()),
        ] {
            h.tx.send(event).unwrap();
        }
        while let Ok(event) = h.rx.try_recv() {
            h.agent.handle(3, event).await.unwrap();
        }
        insta::assert_yaml_snapshot!(h.drain(), @"
        busy: false
        ui:
          - HistoryUpdate:
              ToolOutput:
                call_id: call-1
                chunk: early
          - HistoryUpdate:
              FailUnresolved:
                message: aborted by user
                ended_at: 3
        tasks: []
        ");
        insta::assert_snapshot!(call_output(&h).unwrap(), @r#"{"error":"aborted by user; partial output:\nearly"}"#);
    }

    /// a turn aborted after its stream completed keeps the success: the
    /// failure is a no-op, for the app mirror too
    #[tokio::test]
    async fn abort_after_completed_stream_keeps_success() {
        let mut h = Harness::with(&[]).await;
        let turn = h.step(1, submit("hi")).await.1.task();
        h.step(2, AgentEvent::Stream(turn, text_output("out", "done")))
            .await
            .0
            .unwrap();
        h.step(
            3,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 3 }),
        )
        .await
        .0
        .unwrap();
        assert_handled!(h, 4, user(UserCommand::Abort), @"
        busy: false
        ui:
          - HistoryUpdate:
              FailUnresolved:
                message: aborted by user
                ended_at: 4
        tasks: []
        ");
        insta::assert_yaml_snapshot!(h.history().state().status(), @"Success");
    }

    /// a turn error after its stream completed is surfaced, and the
    /// response keeps its success
    #[tokio::test]
    async fn turn_error_after_completed_stream_keeps_success() {
        let mut h = Harness::with(&[]).await;
        let turn = h.step(1, submit("hi")).await.1.task();
        h.step(
            2,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 2 }),
        )
        .await
        .0
        .unwrap();
        assert_handled!(h, 3, AgentEvent::Done(turn, Err("stream closed".into())), @"
        busy: false
        ui:
          - Error: stream closed
          - HistoryUpdate:
              TurnResponse:
                Failed:
                  message: stream closed
                  ended_at: 3
        tasks: []
        ");
        insta::assert_yaml_snapshot!(h.history().state().status(), @"Success");
    }

    /// a restart fails whatever was in flight, keeping the output saved so
    /// far, so the restored agent takes the next prompt as usual
    #[tokio::test]
    async fn restart_fails_inflight_response_and_tool_calls() {
        let mut h = Harness::with(&[]).await;
        let turn = h.step(1, submit("hi")).await.1.task();
        let tool = h
            .step(2, AgentEvent::Stream(turn, todo_call(None)))
            .await
            .1
            .task();
        h.step(3, AgentEvent::Output(tool, "early".into()))
            .await
            .0
            .unwrap();
        // output alone isn't saved: the next save takes it along
        h.step(4, message("ping")).await.0.unwrap();

        let mut restored = h.restart().await;
        insta::assert_yaml_snapshot!(
            restored.history().state().messages,
            { "[1].ended_at" => "[now]" },
            @r#"
        - role: user
          text: hi
          token_count: 1
          created_at: 1
        - role: assistant
          status:
            Error: lost on restart
          content:
            - - call-1
              - ToolCall:
                  id: call-1
                  call_id: call-1
                  name: todo
                  arguments:
                    current: ""
                    entries: []
                  meta: ~
                  output:
                    Err: "lost on restart; partial output:\nearly"
                  token_count: 29
                  started_at: 2
                  ended_at: 3
                  ready_at: ~
          token_count: 29
          created_at: 1
          started_at: ~
          ended_at: "[now]"
          ready_at: ~
        "#
        );
        let (result, step) = restored.step(5, submit("again")).await;
        result.unwrap();
        assert!(step.starts_turn());
    }

    #[tokio::test]
    async fn task_failure_emits_error_and_fails_turn() {
        let mut h = Harness::with(&[]).await;
        let history = h.history_mut();
        history
            .handle(HistoryUpdate::UserMessage(UserMessage::new(
                "first".into(),
                0,
            )))
            .unwrap();
        history
            .handle(HistoryUpdate::TurnResponse(AssistantEvent::Created {
                created_at: 0,
            }))
            .unwrap();
        history
            .handle(HistoryUpdate::TurnResponse(AssistantEvent::Failed {
                message: "oops".into(),
                ended_at: 1,
            }))
            .unwrap();
        let tid = h.tasks.register(Task::Turn);

        assert_handled!(h, 7, AgentEvent::Done(tid, Err("oops".into())), @"
        busy: false
        ui:
          - Error: oops
          - HistoryUpdate:
              TurnResponse:
                Failed:
                  message: oops
                  ended_at: 7
        tasks: []
        ");
    }

    #[tokio::test]
    async fn set_assistant_rejected_while_busy() {
        let mut h = Harness::with(&[]).await;
        h.tasks.register(Task::Turn);
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
        h.tasks.register(Task::Turn);
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
        ui: []
        tasks: []
        ");
    }

    /// abort already resolved the task: whatever it sent after, failures
    /// included, is dropped
    #[tokio::test]
    async fn task_events_after_abort_are_dropped() {
        let mut h = Harness::with(&[]).await;
        let tid = h.step(1, submit("hi")).await.1.task();
        h.step(2, user(UserCommand::Abort)).await.0.unwrap();

        assert_handled!(h, 3, AgentEvent::Stream(tid, text_output("out", "late")), @"
        busy: false
        ui: []
        tasks: []
        ");
        assert_handled!(h, 4, AgentEvent::Done(tid, Err("stream closed".into())), @"
        busy: false
        ui: []
        tasks: []
        ");
    }

    #[tokio::test]
    async fn task_done_starts_followup_turn_after_tool_resolves() {
        let mut h = Harness::with(&[]).await;
        let turn = h.step(1, submit("hi")).await.1.task();

        let (result, step) = h.step(2, AgentEvent::Stream(turn, todo_call(None))).await;
        result.unwrap();
        let tool = step.task();
        insta::assert_yaml_snapshot!(step, @r#"
        busy: true
        ui:
          - HistoryUpdate:
              TurnResponse:
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

        // the tool task's terminal resolves the slot and starts the follow-up
        assert_handled!(
            h, 6,
            AgentEvent::Done(tool, Ok(TaskOutput::Tool(Box::new(todo_item(Some(Ok(TodoResult {}))))))),
            @r#"
        busy: true
        ui:
          - HistoryUpdate:
              TurnResponse:
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
              TurnResponse:
                Created:
                  created_at: 6
        tasks:
          - - 2
            - turn
        "#
        );
    }

    #[tokio::test]
    async fn tool_failure_patches_slot_and_starts_followup() {
        let mut h = Harness::with(&[]).await;
        let turn = h.step(1, submit("hi")).await.1.task();
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
        // tasks unstick, and the model sees the failure next turn
        assert_handled!(
            h, 5,
            AgentEvent::Done(tool, Err("tool panicked: boom".into())),
            @r#"
        busy: true
        ui:
          - HistoryUpdate:
              ToolCallFailed:
                call_id: call-1
                error: "tool panicked: boom"
          - HistoryUpdate:
              TurnResponse:
                Created:
                  created_at: 5
        tasks:
          - - 2
            - turn
        "#
        );
        // the slot itself holds the error (the follow-up Created appended a
        // fresh assistant message after it)
        insta::assert_snapshot!(call_output(&h).unwrap(), @r#"{"error":"tool panicked: boom; partial output:\nabc"}"#);
    }

    #[tokio::test]
    async fn message_while_idle_appends_and_wakes() {
        let mut h = Harness::with(&[]).await;
        assert_handled!(h, 7, message("hi"), @r#"
        busy: true
        ui:
          - HistoryUpdate:
              DeveloperMessage:
                Peer:
                  text: "[from: kid]\nhi"
                  token_count: 5
                  created_at: 5
          - HistoryUpdate:
              TurnResponse:
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
        let turn = h.step(1, submit("hi")).await.1.task();

        // turn still streaming: buffered, save only
        assert_handled!(h, 2, message("early bird"), @"
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
              DeveloperMessage:
                Peer:
                  text: "[from: kid]\nearly bird"
                  token_count: 6
                  created_at: 5
          - HistoryUpdate:
              TurnResponse:
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
        assert_rejected!(h, 5, message("hi"), @r#"
        - "unknown assistant \"gone\""
        - busy: false
          ui:
            - HistoryUpdate:
                DeveloperMessage:
                  Peer:
                    text: "[from: kid]\nhi"
                    token_count: 5
                    created_at: 5
          tasks: []
        "#);
        let saved = h.project.store().load_state(&h.id).await.unwrap();
        assert!(format!("{:?}", saved.history).contains("[from: kid]"));
    }

    #[tokio::test]
    async fn submit_flushes_stranded_buffer_before_user_message() {
        let mut h = Harness::with(&[]).await;
        h.step(1, submit("hi")).await.0.unwrap();
        h.step(2, message("stranded")).await.0.unwrap();
        h.step(3, user(UserCommand::Abort)).await.0.unwrap();

        let (result, step) = h.step(4, submit("continue")).await;
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
            "peer message must precede the user message"
        );
        assert!(h.state.pending_messages.is_empty());
    }

    #[tokio::test]
    async fn buffered_message_survives_restore_and_flushes_on_submit() {
        let mut h = Harness::with(&[]).await;
        h.step(1, submit("hi")).await.0.unwrap();
        h.step(2, message("persisted")).await.0.unwrap();
        h.step(3, user(UserCommand::Abort)).await.0.unwrap();

        let mut restored = h.restart().await;
        assert_eq!(restored.state.pending_messages.len(), 1);

        let (result, step) = restored.step(5, submit("go")).await;
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
        let tid = h.step(1, submit("hi")).await.1.task();
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
        let tid = h.step(1, submit("hi")).await.1.task();
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
              TurnResponse:
                Failed:
                  message: boom
                  ended_at: 3
        tasks: []
        ");
    }

    #[tokio::test]
    async fn retry_failed_turn_starts_normal_turn() {
        let mut h = Harness::with(&[]).await;
        let history = h.history_mut();
        history
            .handle(HistoryUpdate::UserMessage(UserMessage::new("hi".into(), 0)))
            .unwrap();
        history
            .handle(HistoryUpdate::TurnResponse(AssistantEvent::Created {
                created_at: 0,
            }))
            .unwrap();
        history
            .handle(HistoryUpdate::TurnResponse(AssistantEvent::Failed {
                message: "oops".into(),
                ended_at: 1,
            }))
            .unwrap();

        assert_handled!(h, 7, user(UserCommand::Retry), @"
        busy: true
        ui:
          - HistoryUpdate:
              TurnResponse:
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
              Pop: 1
        tasks: []
        ");
        assert_eq!(h.history().state().messages.len(), 1);
    }

    #[tokio::test]
    async fn compact_runs_alongside_turn_and_applies_at_turn_end() {
        let mut h = Harness::with(&["first"]).await;
        let turn = h.step(1, submit("hi")).await.1.task();

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
            h.tasks.get(compact),
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
              Compact:
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
              Compact:
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

        let (result, step) = h.step(2, submit("hi")).await;
        result.unwrap();
        assert!(step.starts_turn());
        assert!(h.tasks.in_turn() && h.tasks.compacting());
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
        assert!(h.tasks.idle());
    }

    #[tokio::test]
    async fn abort_drops_compaction() {
        let mut h = Harness::with(&["first"]).await;
        h.step(1, submit("hi")).await.0.unwrap();
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
        h.step(1, submit("hi")).await.0.unwrap();
        h.step(2, message("stranded")).await.0.unwrap();
        h.step(3, user(UserCommand::Abort)).await.0.unwrap();
        let compact = h.step(4, user(UserCommand::Compact(1))).await.1.summary();

        let (result, step) = h.step(5, AgentEvent::Done(compact, summary("gist"))).await;
        result.unwrap();

        assert!(!step.starts_turn());
        assert_eq!(h.state.pending_messages.len(), 1);
        assert!(h.tasks.idle());
    }

    #[tokio::test]
    async fn autocompact_past_threshold_runs_alongside_the_turn() {
        let mut h = near_full(100).await;

        let (result, step) = h.step(1, submit("hi")).await;
        result.unwrap();

        let compact = step.summary();
        assert!(matches!(
            h.tasks.get(compact),
            Some(Task::Compact { n_drop: 1 })
        ));
        assert!(step.starts_turn());
    }

    #[tokio::test]
    async fn autocompact_below_threshold_does_nothing() {
        let mut h = near_full(100).await;
        h.project.config_mut().compact.threshold = 90;

        let (result, step) = h.step(1, submit("hi")).await;
        result.unwrap();

        assert!(!h.tasks.compacting());
        assert!(step.starts_turn());
    }

    #[tokio::test]
    async fn hard_limit_holds_turn_until_summary_lands() {
        let mut h = near_full(50).await;

        let (result, step) = h.step(1, submit("hi")).await;
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
        let (result, step) = h.step(2, submit("more")).await;
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

    /// a failed summary blocks the held turn, through aborts and retries,
    /// until a manual compaction brings the history back under the limit
    #[tokio::test]
    async fn failed_summary_blocks_held_turn_until_manual_compaction() {
        let mut h = near_full(50).await;
        let compact = h.step(1, submit("hi")).await.1.summary();

        assert_rejected!(h, 2, AgentEvent::Done(compact, Err("rate limited".into())), @r#"
        - "past the hard limit, compaction failed: rate limited"
        - busy: false
          ui:
            - Error: rate limited
          tasks: []
        "#);
        h.step(3, user(UserCommand::Abort)).await.0.unwrap();
        assert_rejected!(h, 4, user(UserCommand::Retry), @r#"
        - "past the hard limit, compaction failed: rate limited"
        - busy: false
          ui: []
          tasks: []
        "#);

        let compact = h.step(5, user(UserCommand::Compact(1))).await.1.summary();
        assert!(h.compaction.is_none());
        let (result, step) = h.step(6, AgentEvent::Done(compact, summary("gist"))).await;
        result.unwrap();
        assert!(step.starts_turn());
    }

    /// a summary failing alongside a turn turns autocompact off: later
    /// turns start without one, until a manual compaction
    #[tokio::test]
    async fn failed_autocompact_stays_off_until_manual_compaction() {
        let mut h = near_full(100).await;
        let step = h.step(1, submit("hi")).await.1;
        let turn = step.tasks.iter().find(|t| t.1 == "turn").unwrap().0;
        h.step(
            2,
            AgentEvent::Done(step.summary(), Err("rate limited".into())),
        )
        .await
        .0
        .unwrap();
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

        let step = h.step(5, submit("again")).await.1;
        insta::assert_yaml_snapshot!(step.tasks, @"
        - - 2
          - turn
        ");
        h.step(6, user(UserCommand::Compact(1))).await.1.summary();
        assert!(h.compaction.is_none());
    }

    #[tokio::test]
    async fn past_hard_limit_with_nothing_to_compact_blocks() {
        let mut h = near_full(50).await;
        // already under the target: autocompact drops nothing
        h.project.config_mut().compact.target = 100;
        assert_rejected!(h, 1, submit("hi"), @r#"
        - "past the hard limit, nothing to compact"
        - busy: false
          ui:
            - HistoryUpdate:
                UserMessage:
                  text: hi
                  token_count: 1
                  created_at: 1
          tasks: []
        "#);
        assert!(!h.busy());
    }

    /// a summary failing alongside a turn keeps the turn a mid-turn message
    /// made due: the message flushes into it at turn end
    #[tokio::test]
    async fn failed_summary_mid_turn_keeps_due_turn() {
        let mut h = Harness::with(&["first"]).await;
        let turn = h.step(1, submit("hi")).await.1.task();
        let compact = h.step(2, user(UserCommand::Compact(1))).await.1.summary();
        h.step(3, message("report")).await.0.unwrap();
        assert_handled!(h, 4, AgentEvent::Done(compact, Err("rate limited".into())), @"
        busy: true
        ui:
          - Error: rate limited
        tasks: []
        ");

        h.step(
            5,
            AgentEvent::Stream(turn, AssistantEvent::Completed { ended_at: 5 }),
        )
        .await
        .0
        .unwrap();
        assert_handled!(h, 6, AgentEvent::Done(turn, Ok(TaskOutput::Turn)), @r#"
        busy: true
        ui:
          - HistoryUpdate:
              DeveloperMessage:
                Peer:
                  text: "[from: kid]\nreport"
                  token_count: 5
                  created_at: 5
          - HistoryUpdate:
              TurnResponse:
                Created:
                  created_at: 6
        tasks:
          - - 2
            - turn
        "#);
        assert!(h.state.pending_messages.is_empty());
    }
}

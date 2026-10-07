//! full agent-loop tests: scripted assistant turns through `FakeApi`, driven
//! through the real `Agent::handle` event loop
#![cfg(test)]

use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::timeout;

use crate::agent::Agent;
use crate::agent::event::AgentEvent;
use crate::agent::event::UiEvent;
use crate::agent::event::UserCommand;
use crate::agent::event::UserPrompt;
use crate::agent::router::api::RouterError;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::agent::tool::traits::ToolCall;
use crate::agent::tool::traits::ToolCallSerializable;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::delta::Delta;
use crate::llm::history::delta::DeltaContent;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::AssistantStatus;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::OutputItem;
use crate::llm::history::message::ToolCallItem;
use crate::llm::history::message::UserMessage;
use crate::tools::todo::TodoArguments;
use crate::tools::todo::TodoCall;
use crate::tui::app::AppEvent;
use crate::tui::widgets::container::element::Element;
use crate::utils::now;

const TIMEOUT: Duration = Duration::from_secs(5);

fn output(
    id: &str,
    started_at: u64,
) -> AssistantEvent {
    AssistantEvent::Item(Box::new(AssistantItem::Output(OutputItem::new(
        id.into(),
        started_at,
    ))))
}

fn delta(
    id: &str,
    text: &str,
    timestamp: u64,
) -> AssistantEvent {
    AssistantEvent::Delta(Delta::new_at(
        id.into(),
        DeltaContent::Output(text.into()),
        timestamp,
    ))
}

/// scriptable streaming tool: emits chunks through the ctx output sink, then
/// optionally hangs (abort target) or panics; registered with typetag but not
/// with the inventory registry, so the real tool set stays untouched
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct StreamTestCall {
    chunks: Vec<String>,
    hang: bool,
    panic_after: bool,
    output: Option<Result<String, String>>,
}

#[typetag::serde(name = "stream_test")]
impl ToolCallSerializable for StreamTestCall {}

impl From<&StreamTestCall> for Element {
    fn from(_: &StreamTestCall) -> Self {
        Element::default()
    }
}

#[async_trait::async_trait]
impl ToolCall for StreamTestCall {
    fn arguments(&self) -> String {
        "{}".into()
    }

    fn output(&self) -> Option<String> {
        self.output
            .as_ref()
            .map(|o| serde_json::to_string(o).unwrap())
    }

    async fn run(
        &mut self,
        ctx: ToolRuntimeContext,
    ) {
        for chunk in &self.chunks {
            ctx.sink.output(chunk.clone());
        }
        if self.hang {
            std::future::pending::<()>().await;
        }
        if self.panic_after {
            panic!("stream test panic");
        }
        self.output = Some(Ok("streamed".into()));
    }

    fn fail_unresolved(
        &mut self,
        msg: &str,
    ) {
        if self.output.is_none() {
            self.output = Some(Err(msg.to_string()));
        }
    }

    fn compose(
        &mut self,
        streamed: String,
    ) {
        self.output = Some(Ok(streamed));
    }
}

fn tool_call(
    call_id: &str,
    task: Box<dyn ToolCallSerializable>,
) -> AssistantEvent {
    AssistantEvent::Item(Box::new(AssistantItem::ToolCall(ToolCallItem {
        id: Some(call_id.into()),
        call_id: call_id.into(),
        task,
        token_count: 0,
        started_at: 2,
        ended_at: Some(3),
        ready_at: None,
    })))
}

fn stream_call(
    call_id: &str,
    chunks: &[&str],
    hang: bool,
    panic_after: bool,
) -> AssistantEvent {
    tool_call(
        call_id,
        Box::new(StreamTestCall {
            chunks: chunks.iter().map(|c| (*c).to_string()).collect(),
            hang,
            panic_after,
            output: None,
        }),
    )
}

/// tool-output chunks forwarded to the app so far, asserting the call tag
fn drain_chunks(
    app_rx: &mut UnboundedReceiver<AppEvent>,
    call_id: &str,
) -> Vec<String> {
    let mut chunks = Vec::new();
    while let Ok(event) = app_rx.try_recv() {
        if let AppEvent::Agent(_, UiEvent::ToolOutput { call_id: id, chunk }) = event {
            assert_eq!(id, call_id);
            chunks.push(chunk);
        }
    }
    chunks
}

fn todo_call(call_id: &str) -> AssistantEvent {
    tool_call(
        call_id,
        Box::new(TodoCall {
            arguments: Some(TodoArguments::default()),
            meta: None,
            output: None,
        }),
    )
}

fn submit() -> AgentEvent {
    AgentEvent::User(UserCommand::Submit(UserPrompt {
        text: "hi".into(),
        generation: Some(0),
    }))
}

fn submit_text(text: &str) -> AgentEvent {
    AgentEvent::User(UserCommand::Submit(UserPrompt {
        text: text.into(),
        generation: None,
    }))
}

/// serialized output of the given call's history slot, if resolved
fn slot_output(
    agent: &Agent,
    call_id: &str,
) -> Option<String> {
    agent
        .history()
        .state()
        .iter()
        .filter_map(|m| m.try_as_assistant_ref())
        .flat_map(|m| m.content.values())
        .find_map(|item| match item {
            AssistantItem::ToolCall(call) if call.call_id == call_id => call.task.output(),
            _ => None,
        })
}

/// register the test-driven agent with its (real) router, so mail sent to
/// it lands in the channel the test pumps
fn register(agent: &Agent) {
    agent.router.attach_manual(&agent.id, agent.task_tx.clone());
    agent.report_status();
}

/// pump the agent's own event loop until the predicate holds
async fn pump_until(
    agent: &mut Agent,
    pred: impl Fn(&Agent) -> bool,
) {
    timeout(TIMEOUT, async {
        while !pred(agent) {
            let event = agent.next_event().await.unwrap();
            let _ = agent.handle(now(), event).await.unwrap();
        }
    })
    .await
    .expect("timed out pumping agent events");
}

macro_rules! assert_messages_snapshot {
    ($messages:expr, @$snapshot:literal) => {
        insta::assert_yaml_snapshot!($messages, {
            ".**.created_at" => "[ts]",
            ".**.started_at" => "[ts]",
            ".**.ended_at" => "[ts]",
            ".**.ready_at" => "[ts]",
            ".**.timestamp" => "[ts]",
        }, @$snapshot);
    };
}

#[tokio::test]
async fn submit_runs_tool_call_and_second_turn_to_idle() {
    let (mut agent, fake, _parent_rx) = Agent::fake("loop-happy").await;
    register(&agent);
    fake.script_turn(vec![
        output("out-1", 1),
        delta("out-1", "let me check", 2),
        todo_call("call-1"),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("out-2", 4),
        delta("out-2", "all done", 5),
        AssistantEvent::Completed { ended_at: 6 },
    ]);

    let _ = agent.handle(now(), submit()).await.unwrap();
    pump_until(&mut agent, |a| a.tasks.idle()).await;

    assert!(!agent.busy());
    // second request must carry the executed tool call back to the assistant
    assert_eq!(fake.requests().len(), 2);
    assert_messages_snapshot!(&agent.history().state().messages, @r#"
    - role: user
      text: hi
      token_count: 1
      created_at: "[ts]"
    - role: assistant
      status: Success
      content:
        - - out-1
          - Output:
              id: out-1
              content:
                - Text: let me check
              token_count: 3
              started_at: "[ts]"
              ended_at: "[ts]"
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
                Ok: {}
              token_count: 17
              started_at: "[ts]"
              ended_at: "[ts]"
              ready_at: "[ts]"
      token_count: 20
      created_at: "[ts]"
      started_at: "[ts]"
      ended_at: "[ts]"
      ready_at: "[ts]"
    - role: assistant
      status: Success
      content:
        - - out-2
          - Output:
              id: out-2
              content:
                - Text: all done
              token_count: 2
              started_at: "[ts]"
              ended_at: "[ts]"
      token_count: 2
      created_at: "[ts]"
      started_at: "[ts]"
      ended_at: "[ts]"
      ready_at: "[ts]"
    "#);
}

#[tokio::test]
async fn abort_mid_stream_fails_turn_and_goes_idle() {
    let (mut agent, fake, _parent_rx) = Agent::fake("loop-abort").await;
    register(&agent);
    fake.script_hanging_turn(vec![output("out-1", 1), delta("out-1", "partial", 2)]);

    let _ = agent.handle(now(), submit()).await.unwrap();
    pump_until(&mut agent, |a| {
        a.history()
            .state()
            .last()
            .and_then(|m| m.try_as_assistant_ref())
            .is_some_and(|m| m.text_output() == "partial")
    })
    .await;

    let _ = agent
        .handle(now(), AgentEvent::User(UserCommand::Abort))
        .await
        .unwrap();

    assert!(!agent.busy());
    assert_messages_snapshot!(&agent.history().state().messages, @r#"
    - role: user
      text: hi
      token_count: 1
      created_at: "[ts]"
    - role: assistant
      status:
        Error: aborted by user
      content:
        - - out-1
          - Output:
              id: out-1
              content:
                - Text: partial
              token_count: 1
              started_at: "[ts]"
              ended_at: "[ts]"
      token_count: 1
      created_at: "[ts]"
      started_at: "[ts]"
      ended_at: "[ts]"
      ready_at: "[ts]"
    "#);
}

#[tokio::test]
async fn tool_output_streams_to_app_and_terminal_item_resolves() {
    let (mut agent, fake, mut app_rx) = Agent::fake("loop-stream").await;
    register(&agent);
    fake.script_turn(vec![
        stream_call("call-1", &["a", "b"], false, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("out-2", 4),
        delta("out-2", "done", 5),
        AssistantEvent::Completed { ended_at: 6 },
    ]);

    let _ = agent.handle(now(), submit()).await.unwrap();
    pump_until(&mut agent, |a| a.tasks.idle()).await;

    // every chunk reached the app in order, tagged by call id
    assert_eq!(drain_chunks(&mut app_rx, "call-1"), ["a", "b"]);
    // history holds the one terminal item — chunks never became messages
    assert_messages_snapshot!(&agent.history().state().messages, @r#"
    - role: user
      text: hi
      token_count: 1
      created_at: "[ts]"
    - role: assistant
      status: Success
      content:
        - - call-1
          - ToolCall:
              id: call-1
              call_id: call-1
              name: stream_test
              chunks:
                - a
                - b
              hang: false
              panic_after: false
              output:
                Ok: ab
              token_count: 16
              started_at: "[ts]"
              ended_at: "[ts]"
              ready_at: "[ts]"
      token_count: 16
      created_at: "[ts]"
      started_at: "[ts]"
      ended_at: "[ts]"
      ready_at: "[ts]"
    - role: assistant
      status: Success
      content:
        - - out-2
          - Output:
              id: out-2
              content:
                - Text: done
              token_count: 1
              started_at: "[ts]"
              ended_at: "[ts]"
      token_count: 1
      created_at: "[ts]"
      started_at: "[ts]"
      ended_at: "[ts]"
      ready_at: "[ts]"
    "#);
}

#[tokio::test]
async fn abort_mid_tool_output_finalizes_slot_with_partial_output() {
    let (mut agent, fake, mut app_rx) = Agent::fake("loop-stream-abort").await;
    register(&agent);
    fake.script_turn(vec![
        stream_call("call-1", &["par", "tial"], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);

    let _ = agent.handle(now(), submit()).await.unwrap();
    // pump until the turn completed and both chunks reached the app — the
    // tool itself hangs forever
    let mut chunks = Vec::new();
    timeout(TIMEOUT, async {
        while !(chunks.len() == 2
            && matches!(
                agent.history().state().status(),
                Some(AssistantStatus::Success)
            ))
        {
            tokio::select! {
                Some(event) = agent.next_event() => {
                    let _ = agent.handle(now(), event).await.unwrap();
                }
                Some(app_event) = app_rx.recv() => {
                    if let AppEvent::Agent(_, UiEvent::ToolOutput { chunk, .. }) = app_event {
                        chunks.push(chunk);
                    }
                }
            }
        }
    })
    .await
    .expect("timed out waiting for streamed chunks");
    assert_eq!(chunks, ["par", "tial"]);

    let _ = agent
        .handle(now(), AgentEvent::User(UserCommand::Abort))
        .await
        .unwrap();

    // abort returned with the call already finalized: partial output kept,
    // marked aborted, tasks unstuck
    assert!(agent.tasks.idle());
    assert!(!agent.busy());
    assert_messages_snapshot!(&agent.history().state().messages, @r#"
    - role: user
      text: hi
      token_count: 1
      created_at: "[ts]"
    - role: assistant
      status: Success
      content:
        - - call-1
          - ToolCall:
              id: call-1
              call_id: call-1
              name: stream_test
              chunks:
                - par
                - tial
              hang: true
              panic_after: false
              output:
                Err: "aborted by user; partial output:\npartial"
              token_count: 25
              started_at: "[ts]"
              ended_at: "[ts]"
              ready_at: "[ts]"
      token_count: 25
      created_at: "[ts]"
      started_at: "[ts]"
      ended_at: "[ts]"
      ready_at: "[ts]"
    "#);
}

/// the abort's emitted history updates must reach the app mirror in an
/// order it accepts — the tool finalization lands at the new generation, so
/// the mirror must learn of the bump first. A mirror seeded from the pre-abort
/// history and fed every emitted update rejects a premature generation.
#[tokio::test]
async fn abort_emits_updates_a_mirror_accepts() {
    let (mut agent, fake, mut app_rx) = Agent::fake("loop-abort-mirror").await;
    register(&agent);
    let mut mirror = agent.history().clone();
    fake.script_turn(vec![
        stream_call("call-1", &["par", "tial"], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);

    agent.handle(now(), submit()).await.unwrap();
    timeout(TIMEOUT, async {
        loop {
            apply_emitted_updates(&mut mirror, &mut app_rx);
            if matches!(
                agent.history().state().status(),
                Some(AssistantStatus::Success)
            ) {
                break;
            }
            let event = agent.next_event().await.unwrap();
            agent.handle(now(), event).await.unwrap();
        }
    })
    .await
    .expect("timed out driving turn");

    agent
        .handle(now(), AgentEvent::User(UserCommand::Abort))
        .await
        .unwrap();
    apply_emitted_updates(&mut mirror, &mut app_rx);

    // the mirror reconstructed the aborted slot with its partial output —
    // proof the finalization landed at a generation the mirror already knew
    assert_eq!(
        slot_in_history(&mirror, "call-1"),
        slot_in_history(agent.history(), "call-1"),
    );
    assert!(
        slot_in_history(&mirror, "call-1")
            .is_some_and(|o| o.contains("aborted by user") && o.contains("partial")),
        "mirror slot missing the aborted partial output"
    );
}

/// apply every buffered `HistoryUpdate` the agent emitted into the mirror, in
/// emission order; a rejected update is a generation desync
fn apply_emitted_updates(
    mirror: &mut crate::llm::history::History,
    app_rx: &mut UnboundedReceiver<AppEvent>,
) {
    while let Ok(event) = app_rx.try_recv() {
        if let AppEvent::Agent(_, UiEvent::HistoryUpdate(g, u)) = event {
            mirror
                .handle(g, u)
                .expect("mirror rejected an emitted update (generation desync)");
        }
    }
}

fn slot_in_history(
    history: &crate::llm::history::History,
    call_id: &str,
) -> Option<String> {
    history
        .state()
        .iter()
        .filter_map(|m| m.try_as_assistant_ref())
        .flat_map(|m| m.content.values())
        .find_map(|item| match item {
            AssistantItem::ToolCall(call) if call.call_id == call_id => call.task.output(),
            _ => None,
        })
}

#[tokio::test]
async fn panicking_tool_resolves_once_and_next_turn_sees_the_error() {
    let (mut agent, fake, _parent_rx) = Agent::fake("loop-stream-panic").await;
    register(&agent);
    fake.script_turn(vec![
        stream_call("call-1", &["pa"], false, true),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("out-2", 4),
        delta("out-2", "recovered", 5),
        AssistantEvent::Completed { ended_at: 6 },
    ]);

    let _ = agent.handle(now(), submit()).await.unwrap();
    pump_until(&mut agent, |a| a.tasks.idle()).await;

    // exactly one resolution: the slot failed with the partial output, the
    // tasks unstuck, and the follow-up turn ran on the error
    let last = agent
        .history()
        .state()
        .last()
        .and_then(|m| m.try_as_assistant_ref())
        .unwrap();
    assert!(matches!(last.status, AssistantStatus::Success));
    similar_asserts::assert_eq!(last.text_output(), "recovered");
    assert_eq!(fake.requests().len(), 2);
    let failed_slot = agent
        .history()
        .state()
        .iter()
        .filter_map(|m| m.try_as_assistant_ref())
        .flat_map(|m| m.content.values())
        .find_map(|item| match item {
            AssistantItem::ToolCall(call) => call.task.output(),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        failed_slot,
        r#"{"Err":"tool panicked: stream test panic; partial output:\npa"}"#
    );
}

/// a panicked turn still lands its terminal Failed event, so the history
/// turn terminates (Error) instead of being orphaned InProgress — otherwise
/// the next flush would stack a fresh turn on top of the orphan.
#[tokio::test]
async fn panicking_turn_finalizes_the_history_turn() {
    let (mut agent, fake, _parent_rx) = Agent::fake("loop-turn-panic").await;
    register(&agent);
    fake.script_panicking_turn(vec![output("out-1", 1), delta("out-1", "partial", 2)]);

    agent.handle(now(), submit()).await.unwrap();
    // drive to the turn's terminal; the tasks unstick either way, but the
    // turn's Done must also mark the history turn Error
    pump_until(&mut agent, |a| a.tasks.idle()).await;

    let assistants: Vec<_> = agent
        .history()
        .state()
        .iter()
        .filter_map(|m| m.try_as_assistant_ref())
        .collect();
    assert_eq!(
        assistants.len(),
        1,
        "the panicked turn must not be orphaned then stacked"
    );
    let msg = assistants[0];
    assert!(
        matches!(&msg.status, AssistantStatus::Error(e) if e.contains("turn panicked")),
        "history turn not terminated: {:?}",
        msg.status
    );
    assert!(msg.text_output().contains("partial"));
}

/// give the agent a real worktree at its snapshot, so spawn can resolve
/// revisions in it
async fn checkout(agent: &Agent) -> std::path::PathBuf {
    agent
        .project
        .new_agent_workdir(&agent.state.commit, &agent.id)
        .await
        .unwrap();
    agent.project.agent_workdir(&agent.id)
}

/// `git add -A && git commit` in a worktree
fn commit_all(
    workdir: &std::path::Path,
    message: &str,
) -> String {
    let repo = git2::Repository::open(workdir).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.update_all(["*"], None).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&head])
        .unwrap()
        .to_string()
}

fn spawn_call(
    call_id: &str,
    prompt: &str,
    inherit_context: bool,
    commit: Option<&str>,
) -> AssistantEvent {
    use crate::tools::agent::spawn::SpawnArguments;
    use crate::tools::agent::spawn::SpawnCall;
    tool_call(
        call_id,
        Box::new(SpawnCall {
            arguments: Some(SpawnArguments {
                prompt: prompt.into(),
                inherit_context,
                commit: commit.map(str::to_string),
            }),
            meta: None,
            output: None,
        }),
    )
}

fn spawn_result(
    agent: &Agent,
    call_id: &str,
) -> crate::tools::agent::spawn::SpawnResult {
    serde_json::from_str(&slot_output(agent, call_id).unwrap()).unwrap()
}

/// the whole inter-agent lifecycle through a real router and a real child
/// runtime: spawn → commit/list → archive. Each parent turn ends with
/// a hanging tool, so the parent stays busy (no follow-up turn can steal a
/// scripted child turn) without holding the provider's one concurrency
/// permit; phases are separated by aborts.
#[tokio::test]
async fn spawn_commit_list_archive_lifecycle() {
    use crate::llm::history::message::Message;
    use crate::tools::agent::archive::ArchiveArguments;
    use crate::tools::agent::archive::ArchiveCall;
    use crate::tools::agent::list::ListArguments;
    use crate::tools::agent::list::ListCall;

    let (mut agent, fake, _parent_rx) = Agent::fake("loop-agents").await;
    register(&agent);
    // the spawn tail loads the parent's state; the tool resolves its HEAD
    agent.save().await.unwrap();
    let parent_workdir = checkout(&agent).await;

    // phase 1 — spawn; the child's seed turn answers
    fake.script_turn(vec![
        spawn_call("call-1", "find the magic word", true, None),
        stream_call("hang-1", &[], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("kid-out", 1),
        delta("kid-out", "the magic word is plum", 2),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    let _ = agent.handle(now(), submit()).await.unwrap();
    pump_until(&mut agent, |a| slot_output(a, "call-1").is_some()).await;
    let spawned = spawn_result(&agent, "call-1");
    let child = spawned.id;
    similar_asserts::assert_eq!(spawned.commit, agent.state.commit);

    agent
        .router
        .idle_with_output(&child, "the magic word is plum")
        .await;
    // the seed rode the inbound path: developer-role, tagged with the sender —
    // and the inherited context precedes it
    let seed_request = &fake.requests()[1];
    assert!(seed_request.len() > 1, "inherited context missing");
    let Some(Message::Developer(DeveloperMessage::Peer(seed))) = seed_request.last() else {
        panic!("seed is not a peer message: {seed_request:?}");
    };
    similar_asserts::assert_eq!(
        seed.text,
        format!("[from: {}]\nfind the magic word", agent.id)
    );

    // phase 2 — the child commits; the parent sees its branch, and lists it
    let child_workdir = agent.project.agent_workdir(&child);
    tokio::fs::write(child_workdir.join("note.txt"), "from child\n")
        .await
        .unwrap();
    let work = commit_all(&child_workdir, "note");
    let seen = git2::Repository::open(&parent_workdir)
        .unwrap()
        .revparse_single(&format!("vc-{child}"))
        .unwrap()
        .id()
        .to_string();
    similar_asserts::assert_eq!(seen, work);
    let _ = agent
        .handle(now(), AgentEvent::User(UserCommand::Abort))
        .await
        .unwrap();
    fake.script_turn(vec![
        tool_call(
            "call-3",
            Box::new(ListCall {
                arguments: Some(ListArguments { subtree: true }),
                meta: None,
                output: None,
            }),
        ),
        stream_call("hang-2", &[], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    let _ = agent.handle(now(), submit_text("collect")).await.unwrap();
    pump_until(&mut agent, |a| slot_output(a, "call-3").is_some()).await;
    let list_out = slot_output(&agent, "call-3").unwrap();
    assert!(
        list_out.contains(&child.to_string()) && list_out.contains(&agent.id.to_string()),
        "{list_out}"
    );

    // phase 3 — archive the child; it becomes unreachable
    let _ = agent
        .handle(now(), AgentEvent::User(UserCommand::Abort))
        .await
        .unwrap();
    fake.script_turn(vec![
        tool_call(
            "call-4",
            Box::new(ArchiveCall {
                arguments: Some(ArchiveArguments { id: child.clone() }),
                meta: None,
                output: None,
            }),
        ),
        stream_call("hang-3", &[], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    let _ = agent.handle(now(), submit_text("cleanup")).await.unwrap();
    pump_until(&mut agent, |a| slot_output(a, "call-4").is_some()).await;
    similar_asserts::assert_eq!(slot_output(&agent, "call-4").unwrap(), "null");
    similar_asserts::assert_eq!(
        agent.router.send_message(&agent.id, &child, "hi"),
        Err(RouterError::Unreachable)
    );
}

/// by default a child starts at the parent's HEAD: committed work reaches
/// it, uncommitted and untracked work doesn't — while its inherited history
/// is the conversation as of dispatch
#[tokio::test]
async fn spawn_starts_at_parent_head_without_uncommitted_work() {
    use crate::llm::history::message::Message;

    let (mut agent, fake, _parent_rx) = Agent::fake("loop-capture").await;
    register(&agent);
    agent.save().await.unwrap();
    let parent_workdir = checkout(&agent).await;
    tokio::fs::write(parent_workdir.join("pre.txt"), "v1")
        .await
        .unwrap();
    let head = commit_all(&parent_workdir, "pre");
    tokio::fs::write(parent_workdir.join("pre.txt"), "v2")
        .await
        .unwrap();
    tokio::fs::write(parent_workdir.join("draft.txt"), "untracked")
        .await
        .unwrap();

    fake.script_turn(vec![
        spawn_call("call-1", "read pre.txt", true, None),
        stream_call("hang-1", &[], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("kid-out", 1),
        delta("kid-out", "done", 2),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    let _ = agent
        .handle(now(), submit_text("the marker is PRE-SPAWN"))
        .await
        .unwrap();
    pump_until(&mut agent, |a| slot_output(a, "call-1").is_some()).await;
    let spawned = spawn_result(&agent, "call-1");
    similar_asserts::assert_eq!(spawned.commit, head);

    let child_workdir = agent.project.agent_workdir(&spawned.id);
    similar_asserts::assert_eq!(
        tokio::fs::read_to_string(child_workdir.join("pre.txt"))
            .await
            .unwrap(),
        "v1"
    );
    assert!(!child_workdir.join("draft.txt").exists());

    // history: the seed request opens with the pre-spawn conversation
    agent.router.idle_with_output(&spawned.id, "done").await;
    let seed_request = &fake.requests()[1];
    assert!(
        seed_request
            .iter()
            .any(|m| matches!(m, Message::User(u) if u.text.contains("PRE-SPAWN"))),
        "inherited history missing the pre-spawn message: {seed_request:?}"
    );
}

/// a non-inheriting child gets no parent messages — only the subagent
/// header — and reads its instructions from the context files of its own
/// tree, here at an explicit (abbreviated) commit id
#[tokio::test]
async fn fresh_spawn_at_a_revision_reads_instructions_from_its_tree() {
    let (mut agent, fake, _parent_rx) = Agent::fake("loop-fresh").await;
    register(&agent);
    agent.save().await.unwrap();
    let parent_workdir = checkout(&agent).await;
    // committed after the parent loaded its instructions
    tokio::fs::write(parent_workdir.join("AGENTS.md"), "FRESH-MARKER")
        .await
        .unwrap();
    let marked = commit_all(&parent_workdir, "marker");
    tokio::fs::write(parent_workdir.join("AGENTS.md"), "LATER-MARKER")
        .await
        .unwrap();
    commit_all(&parent_workdir, "later");

    fake.script_turn(vec![
        spawn_call("call-1", "go", false, Some(&marked[..7])),
        stream_call("hang-1", &[], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("kid-out", 1),
        delta("kid-out", "done", 2),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    let _ = agent
        .handle(now(), submit_text("PARENT-ONLY"))
        .await
        .unwrap();
    pump_until(&mut agent, |a| slot_output(a, "call-1").is_some()).await;
    let spawned = spawn_result(&agent, "call-1");
    similar_asserts::assert_eq!(spawned.commit, marked);
    agent.router.idle_with_output(&spawned.id, "done").await;

    assert!(!agent.history().instructions().contains("FRESH-MARKER"));
    let state = agent.project.store().load_state(&spawned.id).await.unwrap();
    let history = &state.history;
    assert!(history.instructions().contains("FRESH-MARKER"));
    let messages = format!("{:?}", history.state().messages);
    assert!(!messages.contains("PARENT-ONLY"));
    assert!(messages.contains("You are a subagent"), "{messages}");
    assert!(!messages.contains("Messages above"), "{messages}");
}

#[tokio::test]
async fn compaction_runs_alongside_tool_loop() {
    let (mut agent, fake, _parent_rx) = Agent::fake("loop-compact").await;
    register(&agent);
    agent
        .history_mut()
        .handle(
            0,
            HistoryUpdate::UserMessage(UserMessage::new("first".into(), 0)),
        )
        .unwrap();
    // one provider slot: requests run in the order they were made
    fake.script_turn(vec![
        todo_call("call-1"),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("sum-1", 1),
        delta("sum-1", "a concise summary", 2),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("out-2", 4),
        delta("out-2", "done", 5),
        AssistantEvent::Completed { ended_at: 6 },
    ]);

    let _ = agent.handle(now(), submit()).await.unwrap();
    let _ = agent
        .handle(now(), AgentEvent::User(UserCommand::Compact(1)))
        .await
        .unwrap();
    assert!(agent.tasks.in_turn() && agent.tasks.compacting());
    pump_until(&mut agent, |a| !a.busy()).await;

    assert_eq!(fake.requests().len(), 3);
    assert_messages_snapshot!(&fake.requests()[1], @r#"
    - role: user
      text: first
      token_count: 1
      created_at: "[ts]"
    - role: user
      text: "Summarize this conversation for future continuation. Keep concrete user requirements, decisions, constraints, file paths, and unresolved work. Be concise and factual. Output plain text only."
      token_count: 35
      created_at: "[ts]"
    "#);
    assert_messages_snapshot!(&agent.history().state().messages, @r#"
    - role: developer
      Compact:
        text: a concise summary
        token_count: 3
        created_at: "[ts]"
        started_at: "[ts]"
        ended_at: "[ts]"
    - role: user
      text: hi
      token_count: 1
      created_at: "[ts]"
    - role: assistant
      status: Success
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
                Ok: {}
              token_count: 17
              started_at: "[ts]"
              ended_at: "[ts]"
              ready_at: "[ts]"
      token_count: 17
      created_at: "[ts]"
      started_at: "[ts]"
      ended_at: "[ts]"
      ready_at: "[ts]"
    - role: assistant
      status: Success
      content:
        - - out-2
          - Output:
              id: out-2
              content:
                - Text: done
              token_count: 1
              started_at: "[ts]"
              ended_at: "[ts]"
      token_count: 1
      created_at: "[ts]"
      started_at: "[ts]"
      ended_at: "[ts]"
      ready_at: "[ts]"
    "#);
}

#[tokio::test]
async fn failed_compaction_keeps_history() {
    let (mut agent, fake, mut parent_rx) = Agent::fake("loop-compact-fail").await;
    register(&agent);
    for text in ["first", "second"] {
        agent
            .history_mut()
            .handle(
                0,
                HistoryUpdate::UserMessage(UserMessage::new(text.into(), 0)),
            )
            .unwrap();
    }
    fake.script_turn(vec![AssistantEvent::Failed {
        message: "rate limited".into(),
        ended_at: 9,
    }]);

    let _ = agent
        .handle(now(), AgentEvent::User(UserCommand::Compact(1)))
        .await
        .unwrap();
    pump_until(&mut agent, |a| a.tasks.idle()).await;

    assert!(!agent.busy());
    assert_eq!(agent.history().state().messages.len(), 2);
    let mut errors = Vec::new();
    while let Ok(AppEvent::Agent(_, event)) = parent_rx.try_recv() {
        if let UiEvent::Error(error) = event {
            errors.push(error);
        }
    }
    assert_eq!(errors, vec!["rate limited".to_string()]);
}

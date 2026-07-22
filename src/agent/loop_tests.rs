//! full agent-loop tests: scripted assistant turns through `FakeApi`, driven
//! through the real `Agent::handle` event loop
#![cfg(test)]

use std::time::Duration;

use futures::future::AbortHandle;
use tokio::time::timeout;

use crate::agent::Agent;
use crate::agent::ActivityStatus;
use crate::agent::handle::AgentEvent;
use crate::agent::handle::ExternalEvent;
use crate::agent::handle::ParentEvent;
use crate::agent::handle::UserPrompt;
use crate::agent::router::RuntimeHandle;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::TurnOutcome;
use crate::agent::router::api::WaitResult;
use crate::agent::router::graph::NodeStatus;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::agent::tool::traits::ToolCall;
use crate::agent::tool::traits::ToolCallSerializable;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::TurnStatus;
use crate::llm::history::delta::Delta;
use crate::llm::history::delta::DeltaContent;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::AssistantStatus;
use crate::llm::history::message::OutputItem;
use crate::llm::history::message::ToolCallItem;
use crate::llm::history::message::UserMessage;
use crate::tools::todo::TodoArguments;
use crate::tools::todo::TodoCall;
use crate::tui::app::AppEvent;
use crate::tui::widgets::container::element::Element;

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
            ctx.output.send(chunk.clone()).await;
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
    app_rx: &mut tokio::sync::mpsc::Receiver<AppEvent>,
    call_id: &str,
) -> Vec<String> {
    let mut chunks = Vec::new();
    while let Ok(event) = app_rx.try_recv() {
        if let AppEvent::ParentEvent(_, ParentEvent::ToolOutput { call_id: id, chunk }) = event {
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
    AgentEvent::External(ExternalEvent::Submit(UserPrompt {
        text: "hi".into(),
        generation: Some(0),
    }))
}

fn submit_text(text: &str) -> AgentEvent {
    AgentEvent::External(ExternalEvent::Submit(UserPrompt {
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
        .core
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

/// register the test-driven agent with its (real) router so status pings
/// land on a graph node `wait_idle` can watch
async fn register(agent: &Agent) {
    let (abort, _reg) = AbortHandle::new_pair();
    agent.router.register_root(agent.id.clone()).await.unwrap();
    agent
        .router
        .attach_runtime(
            agent.id.clone(),
            RuntimeHandle::new(agent.tx.clone(), agent.user_tx.clone(), abort),
        )
        .await
        .unwrap();
    agent.ping_status(None).await.unwrap();
}

/// pump the agent's own event loop until the router-visible idle transition
/// fires — the step-1 liveness signal `wait` consumes
async fn drive_until_idle(agent: &mut Agent) -> Result<WaitResult, RouterError> {
    let router = agent.router.clone();
    let wait = router.wait_idle(agent.id.clone());
    let mut wait = std::pin::pin!(wait);
    timeout(TIMEOUT, async {
        loop {
            tokio::select! {
                outcome = &mut wait => return outcome,
                event = agent.next_event() => {
                    let _ = agent.handle(event.unwrap()).await.unwrap();
                }
            }
        }
    })
    .await
    .expect("timed out driving agent")
}

/// pump the agent's own event loop until the predicate holds
async fn pump_until(
    agent: &mut Agent,
    pred: impl Fn(&Agent) -> bool,
) {
    timeout(TIMEOUT, async {
        while !pred(agent) {
            let event = agent.next_event().await.unwrap();
            let _ = agent.handle(event).await.unwrap();
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
    register(&agent).await;
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

    let _ = agent.handle(submit()).await.unwrap();
    let outcome = drive_until_idle(&mut agent).await;

    similar_asserts::assert_eq!(
        outcome,
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("all done".into()),
                error: None,
            },
        })
    );
    assert!(matches!(
        agent.core.state.status,
        ActivityStatus::Normal(TurnStatus::Idle)
    ));
    // second request must carry the executed tool call back to the assistant
    assert_eq!(fake.requests().len(), 2);
    assert_messages_snapshot!(&agent.core.history().state().messages, @r#"
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
    register(&agent).await;
    fake.script_hanging_turn(vec![output("out-1", 1), delta("out-1", "partial", 2)]);

    let _ = agent.handle(submit()).await.unwrap();
    pump_until(&mut agent, |a| {
        a.core
            .history()
            .state()
            .last_text_output()
            .is_ok_and(|text| text == "partial")
    })
    .await;

    let _ = agent
        .handle(AgentEvent::External(ExternalEvent::Abort))
        .await
        .unwrap();

    // the failed turn is idle to the router: waiters fire instead of hanging
    let outcome = drive_until_idle(&mut agent).await;
    assert!(
        matches!(
            &outcome,
            Ok(WaitResult {
                status: NodeStatus::Idle,
                ..
            })
        ),
        "{outcome:?}"
    );
    assert!(matches!(
        &agent.core.state.status,
        ActivityStatus::Normal(TurnStatus::Failed(msg)) if msg == "aborted by user"
    ));
    assert!(agent.core.ledger.idle());
    assert_messages_snapshot!(&agent.core.history().state().messages, @r#"
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
    register(&agent).await;
    fake.script_turn(vec![
        stream_call("call-1", &["a", "b"], false, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("out-2", 4),
        delta("out-2", "done", 5),
        AssistantEvent::Completed { ended_at: 6 },
    ]);

    let _ = agent.handle(submit()).await.unwrap();
    let outcome = drive_until_idle(&mut agent).await;

    similar_asserts::assert_eq!(
        outcome,
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("done".into()),
                error: None,
            },
        })
    );
    // every chunk reached the app in order, tagged by call id
    assert_eq!(drain_chunks(&mut app_rx, "call-1"), ["a", "b"]);
    // terminal resolution drained the accumulator
    assert!(agent.accumulators.is_empty());
    // history holds the one terminal item — chunks never became messages
    assert_messages_snapshot!(&agent.core.history().state().messages, @r#"
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
    register(&agent).await;
    fake.script_turn(vec![
        stream_call("call-1", &["par", "tial"], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);

    let _ = agent.handle(submit()).await.unwrap();
    // pump until the turn completed and both chunks reached the app — the
    // tool itself hangs forever
    let mut chunks = Vec::new();
    timeout(TIMEOUT, async {
        while !(chunks.len() == 2
            && matches!(
                agent.core.history().state().status(),
                Some(AssistantStatus::Success)
            ))
        {
            tokio::select! {
                Some(event) = agent.next_event() => {
                    let _ = agent.handle(event).await.unwrap();
                }
                Some(app_event) = app_rx.recv() => {
                    if let AppEvent::ParentEvent(_, ParentEvent::ToolOutput { chunk, .. }) = app_event {
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
        .handle(AgentEvent::External(ExternalEvent::Abort))
        .await
        .unwrap();

    // abort returned with the call already finalized: partial output kept,
    // marked aborted, ledger unstuck
    assert!(agent.core.ledger.idle());
    assert!(agent.accumulators.is_empty());
    assert!(matches!(
        agent.core.state.status,
        ActivityStatus::Normal(TurnStatus::Idle)
    ));
    assert_messages_snapshot!(&agent.core.history().state().messages, @r#"
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

/// H2: the abort's emitted history updates must reach the app mirror in an
/// order it accepts — the tool finalization lands at the new generation, so
/// the mirror must learn of the bump first. A mirror seeded from the pre-abort
/// history and fed every emitted update rejects a premature generation.
#[tokio::test]
async fn abort_emits_updates_a_mirror_accepts() {
    let (mut agent, fake, mut app_rx) = Agent::fake("loop-abort-mirror").await;
    register(&agent).await;
    let mut mirror = agent.core.history().clone();
    fake.script_turn(vec![
        stream_call("call-1", &["par", "tial"], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);

    agent.handle(submit()).await.unwrap();
    timeout(TIMEOUT, async {
        loop {
            apply_emitted_updates(&mut mirror, &mut app_rx);
            if matches!(
                agent.core.history().state().status(),
                Some(AssistantStatus::Success)
            ) {
                break;
            }
            let event = agent.next_event().await.unwrap();
            agent.handle(event).await.unwrap();
        }
    })
    .await
    .expect("timed out driving turn");

    agent
        .handle(AgentEvent::External(ExternalEvent::Abort))
        .await
        .unwrap();
    apply_emitted_updates(&mut mirror, &mut app_rx);

    // the mirror reconstructed the aborted slot with its partial output —
    // proof the finalization landed at a generation the mirror already knew
    assert_eq!(
        slot_in_history(&mirror, "call-1"),
        slot_in_history(agent.core.history(), "call-1"),
    );
    assert!(
        slot_in_history(&mirror, "call-1")
            .is_some_and(|o| o.contains("aborted by user") && o.contains("partial")),
        "mirror slot missing the aborted partial output"
    );
}

/// apply every buffered `HistoryUpdate` the agent emitted into the mirror, in
/// emission order; a rejected update is a generation desync (H2)
fn apply_emitted_updates(
    mirror: &mut crate::llm::history::History,
    app_rx: &mut tokio::sync::mpsc::Receiver<AppEvent>,
) {
    while let Ok(event) = app_rx.try_recv() {
        if let AppEvent::ParentEvent(_, ParentEvent::HistoryUpdate(g, u)) = event {
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
    register(&agent).await;
    fake.script_turn(vec![
        stream_call("call-1", &["pa"], false, true),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("out-2", 4),
        delta("out-2", "recovered", 5),
        AssistantEvent::Completed { ended_at: 6 },
    ]);

    let _ = agent.handle(submit()).await.unwrap();
    let outcome = drive_until_idle(&mut agent).await;

    // exactly one resolution: the slot failed with the partial output, the
    // ledger unstuck, and the follow-up turn ran on the error
    similar_asserts::assert_eq!(
        outcome,
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("recovered".into()),
                error: None,
            },
        })
    );
    assert_eq!(fake.requests().len(), 2);
    let failed_slot = agent
        .core
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

/// H6: a panicked turn still lands its terminal Failed event, so the history
/// turn terminates (Error) instead of being orphaned InProgress — otherwise
/// the next flush would stack a fresh turn on top of the orphan.
#[tokio::test]
async fn panicking_turn_finalizes_the_history_turn() {
    let (mut agent, fake, _parent_rx) = Agent::fake("loop-turn-panic").await;
    register(&agent).await;
    fake.script_panicking_turn(vec![output("out-1", 1), delta("out-1", "partial", 2)]);

    agent.handle(submit()).await.unwrap();
    // drive to the turn's terminal; the ledger unsticks under both the bug and
    // the fix (via TaskDone), but only the fix marks the history turn Error
    pump_until(&mut agent, |a| a.core.ledger.idle()).await;

    let assistants: Vec<_> = agent
        .core
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

/// the whole inter-agent lifecycle through a real router and a real child
/// runtime: spawn → wait/inspect/list → archive. Each parent turn ends with
/// a hanging tool, so the parent stays busy (no follow-up turn can steal a
/// scripted child turn) without holding the provider's one concurrency
/// permit; phases are separated by aborts.
#[tokio::test]
async fn spawn_wait_inspect_archive_lifecycle() {
    use crate::agent::AgentId;
    use crate::llm::history::message::Message;
    use crate::tools::agent::archive::ArchiveArguments;
    use crate::tools::agent::archive::ArchiveCall;
    use crate::tools::agent::inspect::InspectArguments;
    use crate::tools::agent::inspect::InspectCall;
    use crate::tools::agent::list::ListArguments;
    use crate::tools::agent::list::ListCall;
    use crate::tools::agent::spawn::SpawnArguments;
    use crate::tools::agent::spawn::SpawnCall;
    use crate::tools::agent::spawn::SpawnResult;
    use crate::tools::agent::wait::WaitArguments;
    use crate::tools::agent::wait::WaitCall;

    let (mut agent, fake, _parent_rx) = Agent::fake("loop-agents").await;
    register(&agent).await;
    // the spawn tail loads the parent's state and copies its workdir
    agent.save().await.unwrap();
    tokio::fs::create_dir_all(agent.project.agent_workdir(&agent.id))
        .await
        .unwrap();

    // phase 1 — spawn; the child's seed turn answers
    fake.script_turn(vec![
        tool_call(
            "call-1",
            Box::new(SpawnCall {
                arguments: Some(SpawnArguments {
                    prompt: "find the magic word".into(),
                    inherit_context: true,
                }),
                meta: None,
                output: None,
            }),
        ),
        stream_call("hang-1", &[], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("kid-out", 1),
        delta("kid-out", "the magic word is plum", 2),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    let _ = agent.handle(submit()).await.unwrap();
    pump_until(&mut agent, |a| slot_output(a, "call-1").is_some()).await;
    let child: AgentId =
        serde_json::from_str::<SpawnResult>(&slot_output(&agent, "call-1").unwrap())
            .unwrap()
            .id;

    let outcome = timeout(TIMEOUT, agent.router.wait_idle(child.clone()))
        .await
        .unwrap();
    similar_asserts::assert_eq!(
        outcome,
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("the magic word is plum".into()),
                error: None,
            },
        })
    );
    // the seed rode the inbound path: user-role, tagged with the sender —
    // and the inherited context precedes it
    let seed_request = &fake.requests()[1];
    assert!(seed_request.len() > 1, "inherited context missing");
    let Some(Message::User(seed)) = seed_request.last() else {
        panic!("seed is not a user message: {seed_request:?}");
    };
    similar_asserts::assert_eq!(
        seed.text,
        format!("[from: {}]\nfind the magic word", agent.id)
    );

    // phase 2 — wait + inspect + list against the idle child
    tokio::fs::write(
        agent.project.agent_workdir(&child).join("note.txt"),
        "from child\n",
    )
    .await
    .unwrap();
    let _ = agent
        .handle(AgentEvent::External(ExternalEvent::Abort))
        .await
        .unwrap();
    fake.script_turn(vec![
        tool_call(
            "call-2",
            Box::new(WaitCall {
                arguments: Some(WaitArguments { id: child.clone() }),
                meta: None,
                output: None,
            }),
        ),
        tool_call(
            "call-3",
            Box::new(InspectCall {
                arguments: Some(InspectArguments { id: child.clone() }),
                meta: None,
                output: None,
            }),
        ),
        tool_call(
            "call-4",
            Box::new(ListCall {
                arguments: Some(ListArguments { subtree: true }),
                meta: None,
                output: None,
            }),
        ),
        stream_call("hang-2", &[], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    let _ = agent.handle(submit_text("collect")).await.unwrap();
    pump_until(&mut agent, |a| {
        ["call-2", "call-3", "call-4"]
            .iter()
            .all(|c| slot_output(a, c).is_some())
    })
    .await;

    similar_asserts::assert_eq!(
        slot_output(&agent, "call-2").unwrap(),
        r#"{"status":"Idle","output":"the magic word is plum","error":null}"#
    );
    let inspect_out = slot_output(&agent, "call-3").unwrap();
    assert!(inspect_out.contains(r#""status":"Idle""#), "{inspect_out}");
    assert!(inspect_out.contains("from child"), "{inspect_out}");
    let list_out = slot_output(&agent, "call-4").unwrap();
    assert!(
        list_out.contains(&child.to_string()) && list_out.contains(&agent.id.to_string()),
        "{list_out}"
    );

    // phase 3 — archive the child; it becomes unreachable
    let _ = agent
        .handle(AgentEvent::External(ExternalEvent::Abort))
        .await
        .unwrap();
    fake.script_turn(vec![
        tool_call(
            "call-5",
            Box::new(ArchiveCall {
                arguments: Some(ArchiveArguments { id: child.clone() }),
                meta: None,
                output: None,
            }),
        ),
        stream_call("hang-3", &[], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    let _ = agent.handle(submit_text("cleanup")).await.unwrap();
    pump_until(&mut agent, |a| slot_output(a, "call-5").is_some()).await;
    similar_asserts::assert_eq!(slot_output(&agent, "call-5").unwrap(), "null");
    similar_asserts::assert_eq!(
        agent
            .router
            .send_message(agent.id.clone(), child, "hi".into())
            .await
            .unwrap(),
        Err(RouterError::Unreachable)
    );
}

/// §6 spawn-capture consistency: `spawn` captures the history at dispatch
/// and resolves only once the workdir copy is durable, so an edit made
/// after the call resolves lands in neither the child's workdir nor its
/// inherited history — never in the workdir alone
#[tokio::test]
async fn post_spawn_edit_reaches_neither_child_workdir_nor_history() {
    use crate::agent::AgentId;
    use crate::llm::history::message::Message;
    use crate::tools::agent::spawn::SpawnArguments;
    use crate::tools::agent::spawn::SpawnCall;
    use crate::tools::agent::spawn::SpawnResult;

    let (mut agent, fake, _parent_rx) = Agent::fake("loop-capture").await;
    register(&agent).await;
    agent.save().await.unwrap();
    let parent_workdir = agent.project.agent_workdir(&agent.id);
    tokio::fs::create_dir_all(&parent_workdir).await.unwrap();
    tokio::fs::write(parent_workdir.join("pre.txt"), "v1")
        .await
        .unwrap();

    fake.script_turn(vec![
        tool_call(
            "call-1",
            Box::new(SpawnCall {
                arguments: Some(SpawnArguments {
                    prompt: "read pre.txt".into(),
                    inherit_context: true,
                }),
                meta: None,
                output: None,
            }),
        ),
        stream_call("hang-1", &[], true, false),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    fake.script_turn(vec![
        output("kid-out", 1),
        delta("kid-out", "done", 2),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
    let _ = agent
        .handle(submit_text("the marker is PRE-SPAWN"))
        .await
        .unwrap();
    pump_until(&mut agent, |a| slot_output(a, "call-1").is_some()).await;
    let child: AgentId =
        serde_json::from_str::<SpawnResult>(&slot_output(&agent, "call-1").unwrap())
            .unwrap()
            .id;

    // the turn after spawn edits the parent's tree
    tokio::fs::write(parent_workdir.join("pre.txt"), "v2")
        .await
        .unwrap();
    tokio::fs::write(parent_workdir.join("post.txt"), "late")
        .await
        .unwrap();

    // workdir: the frozen capture, not the parent's live tree
    let child_workdir = agent.project.agent_workdir(&child);
    similar_asserts::assert_eq!(
        tokio::fs::read_to_string(child_workdir.join("pre.txt"))
            .await
            .unwrap(),
        "v1"
    );
    assert!(!child_workdir.join("post.txt").exists());

    // history: the seed request opens with the pre-spawn conversation
    timeout(TIMEOUT, agent.router.wait_idle(child))
        .await
        .unwrap()
        .unwrap();
    let seed_request = &fake.requests()[1];
    assert!(
        seed_request
            .iter()
            .any(|m| matches!(m, Message::User(u) if u.text.contains("PRE-SPAWN"))),
        "inherited history missing the pre-spawn message: {seed_request:?}"
    );
}

#[tokio::test]
async fn compact_failure_then_retry_compacts_history() {
    let (mut agent, fake, _parent_rx) = Agent::fake("loop-compact").await;
    for text in ["first", "second"] {
        agent
            .core
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
    fake.script_turn(vec![
        output("sum-1", 1),
        delta("sum-1", "a concise summary", 2),
        AssistantEvent::Completed { ended_at: 3 },
    ]);

    let _ = agent
        .handle(AgentEvent::External(ExternalEvent::Compact(1)))
        .await
        .unwrap();
    pump_until(&mut agent, |a| {
        matches!(&a.core.state.status, ActivityStatus::Compact(TurnStatus::Failed(msg)) if msg == "rate limited")
    })
    .await;
    assert!(agent.core.history().compacting());

    let _ = agent
        .handle(AgentEvent::External(ExternalEvent::Retry))
        .await
        .unwrap();
    pump_until(&mut agent, |a| {
        a.core.ledger.idle() && !a.core.history().compacting()
    })
    .await;

    assert!(matches!(
        agent.core.state.status,
        ActivityStatus::Normal(TurnStatus::Idle)
    ));
    assert_messages_snapshot!(&agent.core.history().state().messages, @r#"
    - role: developer
      Compact:
        text: a concise summary
        needs_another_turn: false
        token_count: 3
        created_at: "[ts]"
        started_at: "[ts]"
        ended_at: "[ts]"
    - role: user
      text: second
      token_count: 1
      created_at: "[ts]"
    "#);
}

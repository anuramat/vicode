//! full agent-loop tests: scripted assistant turns through `FakeApi`, driven
//! through the real `Agent::handle` event loop

use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::agent::Agent;
use crate::agent::AgentStatus;
use crate::agent::handle::AgentEvent;
use crate::agent::handle::ExternalEvent;
use crate::agent::handle::TurnResult;
use crate::agent::handle::UserPrompt;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::TurnStatus;
use crate::llm::history::delta::Delta;
use crate::llm::history::delta::DeltaContent;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::OutputItem;
use crate::llm::history::message::ToolCallItem;
use crate::llm::history::message::UserMessage;
use crate::tools::todo::TodoArguments;
use crate::tools::todo::TodoCall;

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

fn todo_call(call_id: &str) -> AssistantEvent {
    AssistantEvent::Item(Box::new(AssistantItem::ToolCall(ToolCallItem {
        id: Some(call_id.into()),
        call_id: call_id.into(),
        task: Box::new(TodoCall {
            arguments: Some(TodoArguments::default()),
            meta: None,
            output: None,
        }),
        token_count: 0,
        started_at: 2,
        ended_at: Some(3),
        ready_at: None,
    })))
}

fn submit(done: oneshot::Sender<TurnResult>) -> AgentEvent {
    AgentEvent::External(ExternalEvent::Submit(
        UserPrompt {
            text: "hi".into(),
            multiplier: 1,
            generation: 0,
        },
        Some(done),
    ))
}

/// pump the agent's own event loop until the turn-result oneshot fires
async fn drive_until_done(
    agent: &mut Agent,
    mut done: oneshot::Receiver<TurnResult>,
) -> TurnResult {
    timeout(TIMEOUT, async {
        loop {
            tokio::select! {
                result = &mut done => return result.unwrap(),
                event = agent.rx.recv() => {
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
            let event = agent.rx.recv().await.unwrap();
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
async fn submit_runs_tool_call_and_second_turn_to_done() {
    let (mut agent, fake, _parent_rx) = Agent::fake("loop-happy").await;
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

    let (done_tx, done_rx) = oneshot::channel();
    let _ = agent.handle(submit(done_tx)).await.unwrap();
    let result = drive_until_done(&mut agent, done_rx).await;

    assert!(
        matches!(&result, TurnResult::Success { last_text: Some(text) } if text == "all done"),
        "{result:?}"
    );
    assert!(matches!(
        agent.core.state.status,
        AgentStatus::Normal(TurnStatus::Idle)
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
async fn abort_mid_stream_fails_turn_and_fires_done() {
    let (mut agent, fake, _parent_rx) = Agent::fake("loop-abort").await;
    fake.script_hanging_turn(vec![output("out-1", 1), delta("out-1", "partial", 2)]);

    let (done_tx, done_rx) = oneshot::channel();
    let _ = agent.handle(submit(done_tx)).await.unwrap();
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

    let result = timeout(TIMEOUT, done_rx).await.unwrap().unwrap();
    assert!(
        matches!(&result, TurnResult::Failed(msg) if msg == "aborted by user"),
        "{result:?}"
    );
    assert!(matches!(
        &agent.core.state.status,
        AgentStatus::Normal(TurnStatus::Failed(msg)) if msg == "aborted by user"
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

/// would deadlock before replica spawning moved into the executor task: the
/// parent loop inline-awaited a subagent spawn whose snapshot request only
/// the parent loop itself could answer
#[tokio::test]
async fn multiplier_submit_consolidates_replicas_without_deadlock() {
    use crate::agent::AgentId;
    use crate::agent::AgentState;
    use crate::agent::router::AgentRouter;
    use crate::project::Project;
    use crate::project::layout::LayoutTrait;

    let (project, fake) = Project::new_test().unwrap();
    let (app_tx, mut app_rx) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move { while app_rx.recv().await.is_some() {} });
    let router = AgentRouter::spawn(app_tx, project.clone(), Default::default());

    let aid = AgentId::from(format!("replica-submit-{}", uuid::Uuid::new_v4()));
    let commit = git2::Repository::open(project.root())
        .unwrap()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    project
        .new_agent_workdir(&commit, &aid, true)
        .await
        .unwrap();
    let mut state = AgentState::fake(&project);
    state.context.commit = commit;
    let agent = Agent::new(project.clone(), router.clone(), aid.clone(), state);
    router.register(aid.clone(), agent.spawn()).await.unwrap();

    // both replicas fail fast (skipping the workdir diff); the parent then
    // runs a consolidation turn over their report
    for _ in 0..2 {
        fake.script_turn(vec![AssistantEvent::Failed {
            message: "replica boom".into(),
            ended_at: 1,
        }]);
    }
    fake.script_turn(vec![
        output("sum-1", 1),
        delta("sum-1", "consolidated", 2),
        AssistantEvent::Completed { ended_at: 3 },
    ]);

    let done = router
        .submit_oneshot(
            aid,
            UserPrompt {
                text: "go".into(),
                multiplier: 2,
                generation: 0,
            },
        )
        .await
        .unwrap();
    let result = timeout(TIMEOUT, done)
        .await
        .expect("deadlocked: replica spawn blocked the parent loop")
        .unwrap();
    assert!(
        matches!(&result, TurnResult::Success { last_text: Some(text) } if text == "consolidated"),
        "{result:?}"
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
        matches!(&a.core.state.status, AgentStatus::Compact(TurnStatus::Failed(msg)) if msg == "rate limited")
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
        AgentStatus::Normal(TurnStatus::Idle)
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

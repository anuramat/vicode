//! router unit tests (plan §4 step 1): through the real spawned router task,
//! with real child runtimes on scripted `FakeApi` turns
#![cfg(test)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use futures::future::AbortHandle;
use similar_asserts::assert_eq;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::channel;
use tokio::sync::oneshot;
use tokio::time::timeout;

use super::api::RouterError;
use super::api::TurnOutcome;
use super::api::WaitResult;
use super::graph::NodeStatus;
use super::*;
use crate::agent::Agent;
use crate::agent::handle::ExternalEvent;
use crate::agent::handle::UserPrompt;
use crate::llm::history::AssistantEvent;
use crate::llm::history::History;
use crate::llm::history::delta::Delta;
use crate::llm::history::delta::DeltaContent;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::OutputItem;
use crate::llm::provider::api::fake::FakeApi;

const TIMEOUT: Duration = Duration::from_secs(5);

impl AgentRouter {
    /// Construct a handle backed by dead-letter channels — for tests that
    /// instantiate Agents without running a real router/app.
    pub fn test_handle() -> AgentRouterHandle {
        let (app_tx, app_rx) = channel(CHANNEL_CAPACITY);
        std::mem::forget(app_rx);
        Self::test_handle_with_app_tx(app_tx)
    }

    /// Like `test_handle` but caller controls the app channel so test code can
    /// observe `ParentEvent`s emitted by the agent.
    pub fn test_handle_with_app_tx(app_tx: Sender<AppEvent>) -> AgentRouterHandle {
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        std::mem::forget(rx);
        AgentRouterHandle { tx, app_tx }
    }

    /// Like `test_handle` but caller keeps the command receiver so test code
    /// can observe forwarded events.
    pub fn test_handle_with_rx() -> (AgentRouterHandle, Receiver<RouterCommand>) {
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        let (app_tx, app_rx) = channel(CHANNEL_CAPACITY);
        std::mem::forget(app_rx);
        (AgentRouterHandle { tx, app_tx }, rx)
    }
}

impl AgentRouterHandle {
    /// Abort the agent's live runtime; the node stays reachable and the
    /// persisted state and workdir are untouched.
    pub async fn shutdown(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        let (done, rx) = oneshot::channel();
        self.tx.send(RouterCommand::Shutdown { aid, done }).await?;
        rx.await?
    }

    /// test-facing sync primitive: fire on the target's next idle/dead
    /// transition — the same signal `wait` consumes, minus root/cycle checks
    pub async fn wait_idle(
        &self,
        aid: AgentId,
    ) -> Result<WaitResult, RouterError> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::WaitIdle { aid, done })
            .await
            .expect("router died");
        rx.await.expect("router died")
    }
}

struct Rig {
    project: Project,
    api: Arc<FakeApi>,
    router: AgentRouterHandle,
    primary: AgentId,
    // keep the dummy primary runtime's channels open
    _rx: Receiver<AgentEvent>,
    _user_rx: Receiver<AgentEvent>,
}

impl Rig {
    async fn new(name: &str) -> Self {
        let (project, api) = Project::new_test().unwrap();
        let router = spawn_router(&project, Default::default());
        let (primary, _rx, _user_rx) = register_primary(&project, &router, name).await;
        Self {
            project,
            api,
            router,
            primary,
            _rx,
            _user_rx,
        }
    }

    fn script(
        &self,
        id: &str,
        text: &str,
    ) {
        script_turn(&self.api, id, text);
    }

    /// spawn a child that runs one scripted turn to idle
    async fn spawn_idle_child(
        &self,
        parent: &AgentId,
        text: &str,
    ) -> AgentId {
        self.script(text, text);
        let child = self
            .router
            .spawn_agent(parent.clone(), History::new(String::new()), "go".into())
            .await
            .unwrap();
        let outcome = timeout(TIMEOUT, self.router.wait(parent.clone(), child.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            outcome,
            Ok(WaitResult {
                status: NodeStatus::Idle,
                outcome: TurnOutcome {
                    output: Some(text.into()),
                    error: None,
                },
            })
        );
        child
    }
}

fn script_turn(
    api: &FakeApi,
    id: &str,
    text: &str,
) {
    api.script_turn(vec![
        AssistantEvent::Item(Box::new(AssistantItem::Output(OutputItem::new(
            id.into(),
            1,
        )))),
        AssistantEvent::Delta(Delta::new_at(
            id.into(),
            DeltaContent::Output(text.into()),
            2,
        )),
        AssistantEvent::Completed { ended_at: 3 },
    ]);
}

fn spawn_router(
    project: &Project,
    records: BTreeMap<AgentId, graph::GraphRecord>,
) -> AgentRouterHandle {
    let (app_tx, mut app_rx) = channel(256);
    tokio::spawn(async move { while app_rx.recv().await.is_some() {} });
    let outcomes = records
        .keys()
        .map(|a| (a.clone(), Default::default()))
        .collect();
    AgentRouter::spawn(
        app_tx,
        project.clone(),
        records,
        Default::default(),
        outcomes,
    )
}

/// a primary with a workdir + saved state and a dummy (test-held) runtime
async fn register_primary(
    project: &Project,
    router: &AgentRouterHandle,
    name: &str,
) -> (AgentId, Receiver<AgentEvent>, Receiver<AgentEvent>) {
    let aid = AgentId::from(name.to_string());
    tokio::fs::create_dir_all(project.agent_workdir(&aid))
        .await
        .unwrap();
    project
        .store()
        .save_state(&aid, &project.fake_state())
        .await
        .unwrap();
    let (tx, rx) = channel(8);
    let (user_tx, user_rx) = channel(8);
    let (abort, _reg) = AbortHandle::new_pair();
    router.register_root(aid.clone()).await.unwrap();
    router
        .attach_runtime(aid.clone(), RuntimeHandle::new(tx, user_tx, abort))
        .await
        .unwrap();
    router
        .status(
            aid.clone(),
            graph::StatusPing {
                processed: 0,
                status: NodeStatus::Idle,
                outcome: TurnOutcome {
                    output: None,
                    error: None,
                },
            },
        )
        .await
        .unwrap();
    (aid, rx, user_rx)
}

async fn start_saved_agents(
    project: &Project,
    router: &AgentRouterHandle,
    aids: &[AgentId],
) {
    let mut tasks = Vec::new();
    for aid in aids {
        let state = project.store().load_state(aid).await.unwrap();
        let agent = Agent::new(project.clone(), router.clone(), aid.clone(), state);
        let (runtime, task) = agent.prepare();
        router.attach_runtime(aid.clone(), runtime).await.unwrap();
        tasks.push(task);
    }
    for task in tasks {
        task.launch();
    }
    for aid in aids {
        assert!(matches!(
            timeout(TIMEOUT, router.wait_idle(aid.clone()))
                .await
                .unwrap(),
            Ok(WaitResult { .. })
        ));
    }
}

#[test]
fn free_variant_suffixes_per_name() {
    let mut taken = BTreeSet::new();
    assert_eq!(handle::free_variant("a-b-c", &taken).to_string(), "a-b-c");
    taken.insert(AgentId::from("a-b-c".to_string()));
    assert_eq!(handle::free_variant("a-b-c", &taken).to_string(), "a-b-c-2");
    taken.insert(AgentId::from("a-b-c-2".to_string()));
    assert_eq!(handle::free_variant("a-b-c", &taken).to_string(), "a-b-c-3");
}

#[tokio::test]
async fn spawn_registers_under_parent_and_wait_collects_output() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "child result").await;

    let members = rig
        .router
        .list(rig.primary.clone(), false)
        .await
        .unwrap()
        .unwrap();
    let child_member = members.iter().find(|m| m.id == child).unwrap();
    assert_eq!(child_member.parent, Some(rig.primary.clone()));
    assert_eq!(child_member.status, NodeStatus::Idle);

    // a second wait on the already-idle target fires immediately
    let outcome = rig
        .router
        .wait(rig.primary.clone(), child.clone())
        .await
        .unwrap();
    assert_eq!(
        outcome,
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("child result".into()),
                error: None,
            },
        })
    );
    assert_eq!(
        rig.router
            .inspect(rig.primary.clone(), child)
            .await
            .unwrap(),
        Ok(NodeStatus::Idle)
    );
}

#[tokio::test]
async fn send_bumps_delivered_so_racing_wait_returns_post_message_output() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "first").await;

    rig.script("second", "second");
    let sent = rig
        .router
        .send_message(rig.primary.clone(), child.clone(), "more".into())
        .await
        .unwrap();
    assert_eq!(sent, Ok(()));
    // the delivery is outstanding: this wait must register, not fire on the
    // pre-message idle
    let outcome = timeout(TIMEOUT, rig.router.wait(rig.primary.clone(), child))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome,
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("second".into()),
                error: None,
            },
        })
    );
}

#[tokio::test]
async fn send_to_still_spawning_child_parks_in_minted_mailbox() {
    let rig = Rig::new("prime").await;
    rig.script("o1", "seeded");
    // raw command so we see the child while its tail is still in flight
    let (done, spawn_rx) = oneshot::channel();
    rig.router
        .tx
        .send(RouterCommand::Spawn {
            parent: rig.primary.clone(),
            capture: History::new(String::new()),
            prompt: "go".into(),
            done,
        })
        .await
        .unwrap();
    // registration is synchronous in the router task: the next command sees it
    let members = rig
        .router
        .list(rig.primary.clone(), true)
        .await
        .unwrap()
        .unwrap();
    let child = members.iter().find(|m| m.id != rig.primary).unwrap();
    assert_eq!(child.status, NodeStatus::Spawning);

    // parks behind the seed prompt — never Unreachable, never blocking
    // (busy-buffering of the parked message itself lands with step 3)
    let sent = rig
        .router
        .send_message(rig.primary.clone(), child.id.clone(), "psst".into())
        .await
        .unwrap();
    assert_eq!(sent, Ok(()));
    timeout(TIMEOUT, spawn_rx).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn cross_tab_everything_is_unreachable() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "mine").await;
    let (other, _rx, _user_rx) = register_primary(&rig.project, &rig.router, "other").await;

    assert_eq!(
        rig.router
            .send_message(other.clone(), child.clone(), "hi".into())
            .await
            .unwrap(),
        Err(RouterError::Unreachable)
    );
    assert_eq!(
        rig.router
            .inspect(other.clone(), child.clone())
            .await
            .unwrap(),
        Err(RouterError::Unreachable)
    );
    assert_eq!(
        rig.router.wait(other.clone(), child.clone()).await.unwrap(),
        Err(RouterError::Unreachable)
    );
    assert_eq!(
        rig.router
            .archive(other.clone(), child.clone())
            .await
            .unwrap(),
        Err(RouterError::Unreachable)
    );
    // list shows exactly the caller's tab
    let members = rig
        .router
        .list(other.clone(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].id, other);
    // unknown caller
    let ghost = AgentId::from("nobody".to_string());
    assert_eq!(
        rig.router
            .send_message(ghost.clone(), child, "hi".into())
            .await
            .unwrap(),
        Err(RouterError::Unreachable)
    );
    assert_eq!(rig.router.list(ghost, false).await.unwrap(), None);
}

#[tokio::test]
async fn archive_rejects_self_sibling_and_ancestor() {
    let rig = Rig::new("prime").await;
    let c1 = rig.spawn_idle_child(&rig.primary, "one").await;
    let c2 = rig.spawn_idle_child(&rig.primary, "two").await;

    for (caller, target) in [
        (rig.primary.clone(), rig.primary.clone()), // self
        (c1.clone(), c2.clone()),                   // sibling
        (c1.clone(), rig.primary.clone()),          // ancestor
    ] {
        assert_eq!(
            rig.router.archive(caller, target).await.unwrap(),
            Err(RouterError::NotOwned)
        );
    }
}

#[tokio::test]
async fn archive_reaps_exact_subtree_retaining_rows_and_workdirs() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "child").await;
    let grandchild = rig.spawn_idle_child(&child, "grandchild").await;
    let sibling = rig.spawn_idle_child(&rig.primary, "sibling").await;

    assert_eq!(
        rig.router
            .archive(rig.primary.clone(), child.clone())
            .await
            .unwrap(),
        Ok(())
    );

    // exact subtree unreachable, sibling untouched
    for aid in [&child, &grandchild] {
        assert_eq!(
            rig.router
                .send_message(rig.primary.clone(), aid.clone(), "hi".into())
                .await
                .unwrap(),
            Err(RouterError::Unreachable)
        );
    }
    let members = rig
        .router
        .list(rig.primary.clone(), false)
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<_> = members.iter().map(|m| m.id.clone()).collect();
    let mut expected = vec![rig.primary.clone(), sibling.clone()];
    expected.sort();
    assert_eq!(ids, expected);

    // durable: graph records flipped, state + workdirs retained
    let records = rig.project.store().load_graph().await.unwrap();
    assert!(records[&child].archived);
    assert!(records[&grandchild].archived);
    assert!(!records[&sibling].archived);
    for aid in [&child, &grandchild] {
        rig.project.store().load_state(aid).await.unwrap();
        assert!(rig.project.agent_workdir(aid).exists());
    }
}

#[tokio::test]
async fn archive_tab_reaps_every_member() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "child").await;

    rig.router.archive_tab(rig.primary.clone()).await.unwrap();

    assert_eq!(
        rig.router.list(rig.primary.clone(), false).await.unwrap(),
        None
    );
    let records = rig.project.store().load_graph().await.unwrap();
    assert!(records[&rig.primary].archived);
    assert!(records[&child].archived);
}

#[tokio::test]
async fn restart_starts_all_alive_agents_and_excludes_archived() {
    let rig = Rig::new("prime").await;
    let kept = rig.spawn_idle_child(&rig.primary, "kept").await;
    let archived = rig.spawn_idle_child(&rig.primary, "archived").await;
    rig.router
        .archive(rig.primary.clone(), archived.clone())
        .await
        .unwrap()
        .unwrap();
    let mut kept_state = rig.project.store().load_state(&kept).await.unwrap();
    kept_state
        .pending_messages
        .push(crate::llm::history::message::UserMessage::new(
            "[from: prime]\nresume".into(),
            10,
        ));
    rig.project
        .store()
        .save_state(&kept, &kept_state)
        .await
        .unwrap();
    rig.script("resumed", "resumed");

    let records = rig.project.store().load_graph().await.unwrap();
    let router2 = spawn_router(&rig.project, records);
    start_saved_agents(&rig.project, &router2, &[rig.primary.clone(), kept.clone()]).await;

    assert_eq!(
        router2
            .wait(rig.primary.clone(), kept.clone())
            .await
            .unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("resumed".into()),
                error: None,
            },
        })
    );
    assert_eq!(
        router2
            .inspect(rig.primary.clone(), kept.clone())
            .await
            .unwrap(),
        Ok(NodeStatus::Idle)
    );
    assert_eq!(
        router2
            .send_message(rig.primary.clone(), archived.clone(), "hi".into())
            .await
            .unwrap(),
        Err(RouterError::Unreachable)
    );
    assert_eq!(
        router2.wait(rig.primary, archived.clone()).await.unwrap(),
        Err(RouterError::Unreachable)
    );
    rig.project.store().load_state(&archived).await.unwrap();
    assert!(rig.project.agent_workdir(&archived).exists());
}

#[tokio::test]
async fn invalid_child_is_dead_while_valid_descendant_starts() {
    let (project, _api) = Project::new_test().unwrap();
    let prime = AgentId::from("prime".to_string());
    let bad = AgentId::from("bad".to_string());
    let descendant = AgentId::from("descendant".to_string());
    let records = [
        (
            prime.clone(),
            graph::GraphRecord {
                root: prime.clone(),
                parent: None,
                archived: false,
            },
        ),
        (
            bad.clone(),
            graph::GraphRecord {
                root: prime.clone(),
                parent: Some(prime.clone()),
                archived: false,
            },
        ),
        (
            descendant.clone(),
            graph::GraphRecord {
                root: prime.clone(),
                parent: Some(bad.clone()),
                archived: false,
            },
        ),
    ]
    .into_iter()
    .collect();
    for aid in [&prime, &descendant] {
        tokio::fs::create_dir_all(project.agent_workdir(aid))
            .await
            .unwrap();
        project
            .store()
            .save_state(aid, &project.fake_state())
            .await
            .unwrap();
    }
    let outcomes = [
        (
            prime.clone(),
            TurnOutcome {
                output: None,
                error: None,
            },
        ),
        (
            bad.clone(),
            TurnOutcome {
                output: None,
                error: Some("corrupt child row".into()),
            },
        ),
        (
            descendant.clone(),
            TurnOutcome {
                output: None,
                error: None,
            },
        ),
    ]
    .into_iter()
    .collect();
    let (app_tx, mut app_rx) = channel(256);
    tokio::spawn(async move { while app_rx.recv().await.is_some() {} });
    let router = AgentRouter::spawn(
        app_tx,
        project.clone(),
        records,
        Default::default(),
        outcomes,
    );
    start_saved_agents(&project, &router, &[prime.clone(), descendant.clone()]).await;

    assert_eq!(
        router.wait(prime.clone(), bad.clone()).await.unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: None,
                error: Some("corrupt child row".into()),
            },
        })
    );
    assert_eq!(
        router.inspect(prime, descendant).await.unwrap(),
        Ok(NodeStatus::Idle)
    );
}

#[tokio::test]
async fn runtime_death_is_terminal_until_restart() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "answer").await;

    rig.router.shutdown(child.clone()).await.unwrap();

    assert_eq!(
        rig.router
            .wait(rig.primary.clone(), child.clone())
            .await
            .unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: Some("answer".into()),
                error: Some("agent runtime cancelled by test".into()),
            },
        })
    );
    assert_eq!(
        rig.router
            .send_message(rig.primary.clone(), child.clone(), "hi".into())
            .await
            .unwrap(),
        Err(RouterError::Unreachable)
    );
    assert!(
        rig.router
            .forward(child.clone(), ExternalEvent::Abort)
            .await
            .is_err()
    );
    let err = rig
        .router
        .spawn_agent(child.clone(), History::new(String::new()), "go".into())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), format!("agent {child} is dead"));
    // A late status ping cannot revive or overwrite a terminal node.
    rig.router
        .status(
            child.clone(),
            graph::StatusPing {
                processed: 99,
                status: NodeStatus::Idle,
                outcome: TurnOutcome {
                    output: Some("ghost".into()),
                    error: None,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        rig.router
            .wait(rig.primary.clone(), child.clone())
            .await
            .unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: Some("answer".into()),
                error: Some("agent runtime cancelled by test".into()),
            },
        })
    );
    let records = rig.project.store().load_graph().await.unwrap();
    assert!(!records[&child].archived);

    let router2 = spawn_router(&rig.project, records);
    start_saved_agents(
        &rig.project,
        &router2,
        &[rig.primary.clone(), child.clone()],
    )
    .await;
    assert_eq!(
        router2.wait(rig.primary, child).await.unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("answer".into()),
                error: None,
            },
        })
    );
}

#[tokio::test]
async fn spawn_errors_at_the_tab_cap_and_archive_frees_a_slot() {
    let (project, api) = Project::new_test().unwrap();
    let prime = AgentId::from("prime".to_string());
    // Boot with the tab already at the cap. Durable members hold slots even
    // before their runtimes attach.
    let member = |i: usize| {
        (
            AgentId::from(format!("m{i}")),
            graph::GraphRecord {
                root: prime.clone(),
                parent: Some(prime.clone()),
                archived: false,
            },
        )
    };
    let records = std::iter::once((
        prime.clone(),
        graph::GraphRecord {
            root: prime.clone(),
            parent: None,
            archived: false,
        },
    ))
    .chain((1..TAB_AGENT_CAP).map(member))
    .collect();
    let router = spawn_router(&project, records);
    tokio::fs::create_dir_all(project.agent_workdir(&prime))
        .await
        .unwrap();
    project
        .store()
        .save_state(&prime, &project.fake_state())
        .await
        .unwrap();
    start_saved_agents(&project, &router, std::slice::from_ref(&prime)).await;

    let err = router
        .spawn_agent(prime.clone(), History::new(String::new()), "go".into())
        .await
        .unwrap_err()
        .to_string();
    // recovery is spelled out — ids may be gone from compacted history
    assert!(err.contains("archive") && err.contains("list"), "{err}");

    // archive frees the slot; the next spawn goes through
    assert_eq!(
        router
            .archive(prime.clone(), AgentId::from("m1".to_string()))
            .await
            .unwrap(),
        Ok(())
    );
    script_turn(&api, "o1", "fits now");
    let child = router
        .spawn_agent(prime.clone(), History::new(String::new()), "go".into())
        .await
        .unwrap();
    assert_eq!(
        timeout(TIMEOUT, router.wait(prime, child))
            .await
            .unwrap()
            .unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("fits now".into()),
                error: None,
            },
        })
    );
}

#[tokio::test]
async fn wait_cycle_returns_would_deadlock_and_stale_edges_prune() {
    let rig = Rig::new("prime").await;
    let a = rig.spawn_idle_child(&rig.primary, "a").await;
    let b = rig.spawn_idle_child(&rig.primary, "b").await;
    // make both busy: each gets a message whose scripted turn hangs — the
    // outstanding delivery alone makes a racing wait register (M2)
    rig.api.script_hanging_turn(vec![]);
    rig.api.script_hanging_turn(vec![]);
    for target in [&a, &b] {
        assert_eq!(
            rig.router
                .send_message(rig.primary.clone(), target.clone(), "work".into())
                .await
                .unwrap(),
            Ok(())
        );
    }

    // a → b edge, receiver kept alive
    let (done, ab_rx) = oneshot::channel();
    rig.router
        .tx
        .send(RouterCommand::Wait {
            caller: a.clone(),
            target: b.clone(),
            done,
        })
        .await
        .unwrap();
    // closing the cycle is rejected, typed
    assert_eq!(
        rig.router.wait(b.clone(), a.clone()).await.unwrap(),
        Err(RouterError::WouldDeadlock)
    );

    // dropped registration = stale edge: pruned, the reverse wait registers
    drop(ab_rx);
    let reverse = tokio::spawn({
        let router = rig.router.clone();
        let (a, b) = (a.clone(), b.clone());
        async move { router.wait(b, a).await.unwrap() }
    });
    // the abort rides the priority user channel: let a's hanging turn start
    // first, or the abort lands before the wake and aborts nothing
    timeout(TIMEOUT, async {
        loop {
            let outcome = rig.router.inspect(rig.primary.clone(), a.clone()).await;
            if matches!(outcome.unwrap(), Ok(NodeStatus::Running)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a never started its hanging turn");
    // abort a's hanging turn: it idles with the typed failure, the cached
    // output intact (M1)
    rig.router.forward(a, ExternalEvent::Abort).await.unwrap();
    assert_eq!(
        timeout(TIMEOUT, reverse).await.unwrap().unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("a".into()),
                error: Some("aborted by user".into()),
            },
        })
    );
}

/// a spawn mints the child's frozen base commit (C_spawn): parented on the
/// spawner's base, pinned by refs/vicode/base/<child>, and recorded in the
/// child's state
#[tokio::test]
async fn spawn_mints_a_pinned_base_ref() {
    let rig = Rig::new("prime").await;
    std::fs::write(
        rig.project.agent_workdir(&rig.primary).join("work.txt"),
        "parent delta\n",
    )
    .unwrap();
    let child = rig.spawn_idle_child(&rig.primary, "done").await;

    let repo = git2::Repository::open(rig.project.root()).unwrap();
    let minted = repo
        .find_reference(&rig.project.base_ref(&child))
        .unwrap()
        .peel_to_commit()
        .unwrap();
    // parented on the spawner's base — the primary's own base commit
    assert_eq!(
        minted.parent(0).unwrap().id().to_string(),
        rig.project.head_commit()
    );
    // the frozen copy is in the tree, and the child's state points at the mint
    assert!(minted.tree().unwrap().get_name("work.txt").is_some());
    let state = rig.project.store().load_state(&child).await.unwrap();
    assert_eq!(state.context.base, minted.id().to_string());
    assert_eq!(state.context.commit, rig.project.head_commit());
}

/// re-spawning an identical tree dedupes to one C_spawn oid (fixed
/// signature/timestamp), with each child still pinning its own ref
#[tokio::test]
async fn concurrent_double_spawn_pins_both_refs_on_one_oid() {
    let rig = Rig::new("prime").await;
    rig.script("one", "one");
    rig.script("two", "two");
    let (a, b) = tokio::join!(
        rig.router.spawn_agent(
            rig.primary.clone(),
            History::new(String::new()),
            "go".into()
        ),
        rig.router.spawn_agent(
            rig.primary.clone(),
            History::new(String::new()),
            "go".into()
        ),
    );
    let (a, b) = (a.unwrap(), b.unwrap());

    let repo = git2::Repository::open(rig.project.root()).unwrap();
    let base = |aid: &AgentId| {
        repo.find_reference(&rig.project.base_ref(aid))
            .unwrap()
            .target()
            .unwrap()
    };
    assert_eq!(base(&a), base(&b));
}

#[tokio::test]
async fn failed_spawn_rolls_back_node_row_record_and_workdir() {
    let rig = Rig::new("prime").await;
    // no workdir to copy → the detached tail fails after registration
    std::fs::remove_dir_all(rig.project.agent_workdir(&rig.primary)).unwrap();

    let result = rig
        .router
        .spawn_agent(
            rig.primary.clone(),
            History::new(String::new()),
            "go".into(),
        )
        .await;
    assert!(result.is_err());

    let members = rig
        .router
        .list(rig.primary.clone(), true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].id, rig.primary);
    let records = rig.project.store().load_graph().await.unwrap();
    assert_eq!(records.len(), 1);
    assert!(!records[&rig.primary].archived);
    assert_eq!(rig.project.store().load_graph().await.unwrap().len(), 1);
}

/// a spawn failing *after* the mint (bad assistant id) rolls the ref back
/// with the state, graph record and dir — no refs/vicode/base residue
#[tokio::test]
async fn failed_spawn_after_mint_leaves_no_base_ref() {
    let rig = Rig::new("prime").await;
    let mut state = rig.project.fake_state();
    state.assistant = "ghost".into();
    rig.project
        .store()
        .save_state(&rig.primary, &state)
        .await
        .unwrap();

    let result = rig
        .router
        .spawn_agent(
            rig.primary.clone(),
            History::new(String::new()),
            "go".into(),
        )
        .await;
    assert!(result.is_err());

    // the state dies in the same redb transaction as the graph record
    assert_eq!(rig.project.store().load_graph().await.unwrap().len(), 1);
    let dirs: Vec<_> = std::fs::read_dir(rig.project.agents())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        dirs,
        vec![std::ffi::OsString::from(rig.primary.to_string())]
    );
    let repo = git2::Repository::open(rig.project.root()).unwrap();
    assert_eq!(
        repo.references_glob("refs/vicode/base/*").unwrap().count(),
        0
    );
}

#[tokio::test]
async fn archive_racing_in_flight_spawn_never_resurrects_the_record() {
    let rig = Rig::new("prime").await;
    rig.script("o1", "never collected");
    let (done, spawn_rx) = oneshot::channel();
    rig.router
        .tx
        .send(RouterCommand::Spawn {
            parent: rig.primary.clone(),
            capture: History::new(String::new()),
            prompt: "go".into(),
            done,
        })
        .await
        .unwrap();
    let members = rig
        .router
        .list(rig.primary.clone(), true)
        .await
        .unwrap()
        .unwrap();
    let child = members
        .iter()
        .find(|m| m.id != rig.primary)
        .unwrap()
        .id
        .clone();

    // archive while the spawn tail is (likely) still in flight
    assert_eq!(
        rig.router
            .archive(rig.primary.clone(), child.clone())
            .await
            .unwrap(),
        Ok(())
    );
    // Let the tail land either way. If archive won before attachment, setup
    // rolls back; if attachment won, the durable archived graph record wins.
    drop(timeout(TIMEOUT, spawn_rx).await);

    let records = rig.project.store().load_graph().await.unwrap();
    assert!(records.get(&child).is_none_or(|record| record.archived));
    assert_eq!(
        rig.router
            .send_message(rig.primary.clone(), child, "hi".into())
            .await
            .unwrap(),
        Err(RouterError::Unreachable)
    );
}

#[tokio::test]
async fn runtime_attachment_is_one_shot_and_forward_is_acknowledged() {
    let (project, _api) = Project::new_test().unwrap();
    let router = spawn_router(&project, Default::default());
    let aid = AgentId::from("dup".to_string());

    let (tx1, _rx1) = channel(8);
    let (user_tx1, mut user_rx1) = channel(1);
    let (abort1, _reg1) = AbortHandle::new_pair();
    router.register_root(aid.clone()).await.unwrap();
    router
        .attach_runtime(aid.clone(), RuntimeHandle::new(tx1, user_tx1, abort1))
        .await
        .unwrap();

    let (tx2, _rx2) = channel(8);
    let (user_tx2, _user_rx2) = channel(8);
    let (abort2, _reg2) = AbortHandle::new_pair();
    assert!(
        router
            .attach_runtime(aid.clone(), RuntimeHandle::new(tx2, user_tx2, abort2),)
            .await
            .is_err()
    );

    // Success is returned only after the event is actually queued.
    router
        .forward(aid.clone(), ExternalEvent::Abort)
        .await
        .unwrap();
    assert!(router.forward(aid, ExternalEvent::Abort).await.is_err());
    let event = timeout(TIMEOUT, user_rx1.recv()).await.unwrap().unwrap();
    assert!(matches!(event, AgentEvent::External(ExternalEvent::Abort)));
}

#[tokio::test]
async fn forward_to_closed_runtime_is_rejected_and_marks_dead() {
    let (project, _api) = Project::new_test().unwrap();
    let router = spawn_router(&project, Default::default());
    let (aid, _rx, user_rx) = register_primary(&project, &router, "prime").await;
    drop(user_rx); // the agent died without the router noticing

    assert!(
        router
            .forward(
                aid.clone(),
                ExternalEvent::Submit(UserPrompt {
                    text: "hello again".into(),
                    generation: None,
                }),
            )
            .await
            .is_err()
    );

    assert_eq!(
        router.wait_idle(aid.clone()).await,
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: None,
                error: Some("agent runtime mailbox closed".into()),
            },
        })
    );
}

/// M2: a send that lands while the target's pre-send idle ping is still in
/// flight must not fire a later wait with the previous turn's output — the
/// delivery seqnums swallow the stale ping
#[tokio::test]
async fn wait_after_mid_idle_send_returns_post_message_output() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "seed").await;
    let ping = |processed, status, output: Option<&str>| graph::StatusPing {
        processed,
        status,
        outcome: TurnOutcome {
            output: output.map(Into::into),
            error: None,
        },
    };

    // the primary's (emulated) turn ends; its idle ping is still in flight…
    rig.router
        .status(rig.primary.clone(), ping(0, NodeStatus::Running, None))
        .await
        .unwrap();
    // …when the child's message is delivered
    assert_eq!(
        rig.router
            .send_message(child.clone(), rig.primary.clone(), "more".into())
            .await
            .unwrap(),
        Ok(())
    );
    // the stale pre-delivery idle ping arrives: processed < delivered, so a
    // wait registered now must not fire on "old"
    rig.router
        .status(rig.primary.clone(), ping(0, NodeStatus::Idle, Some("old")))
        .await
        .unwrap();
    let (done, wait_rx) = oneshot::channel();
    rig.router
        .tx
        .send(RouterCommand::Wait {
            caller: child,
            target: rig.primary.clone(),
            done,
        })
        .await
        .unwrap();
    // the agent catches up: processes the delivery, runs its turn to idle
    rig.router
        .status(rig.primary.clone(), ping(1, NodeStatus::Running, None))
        .await
        .unwrap();
    rig.router
        .status(rig.primary.clone(), ping(1, NodeStatus::Idle, Some("new")))
        .await
        .unwrap();
    assert_eq!(
        timeout(TIMEOUT, wait_rx).await.unwrap().unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("new".into()),
                error: None,
            },
        })
    );
}

#[tokio::test]
async fn duplicate_runtime_down_keeps_the_first_terminal_error() {
    let (project, _api) = Project::new_test().unwrap();
    let router = spawn_router(&project, Default::default());
    let (aid, _rx, _user_rx) = register_primary(&project, &router, "prime").await;

    router
        .runtime_down(aid.clone(), "first failure".into())
        .await
        .unwrap();
    router
        .runtime_down(aid.clone(), "second failure".into())
        .await
        .unwrap();

    assert_eq!(
        timeout(TIMEOUT, router.wait_idle(aid)).await.unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: None,
                error: Some("first failure".into()),
            },
        })
    );
}

#[tokio::test]
async fn forwarded_abort_to_dead_primary_is_rejected() {
    let rig = Rig::new("prime").await;
    rig.router.shutdown(rig.primary.clone()).await.unwrap();

    assert!(
        rig.router
            .forward(rig.primary.clone(), ExternalEvent::Abort)
            .await
            .is_err()
    );

    assert_eq!(
        timeout(TIMEOUT, rig.router.wait_idle(rig.primary.clone()))
            .await
            .unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: None,
                error: Some("agent runtime cancelled by test".into()),
            },
        })
    );
}

/// H4a: a wake whose turn fails to start still pings — the wait fires with
/// the typed handler error instead of hanging forever
#[tokio::test]
async fn failed_wake_fires_wait_with_typed_error() {
    let (project, _api) = Project::new_test().unwrap();
    let prime = AgentId::from("prime".to_string());
    let records = std::iter::once((
        prime.clone(),
        graph::GraphRecord {
            root: prime.clone(),
            parent: None,
            archived: false,
        },
    ))
    .collect();
    let router = spawn_router(&project, records);
    tokio::fs::create_dir_all(project.agent_workdir(&prime))
        .await
        .unwrap();
    // state whose assistant id is gone from the pool: `start_turn` will fail
    let mut state = project.fake_state();
    state.assistant = "gone".into();
    project.store().save_state(&prime, &state).await.unwrap();
    start_saved_agents(&project, &router, std::slice::from_ref(&prime)).await;

    router
        .forward(
            prime.clone(),
            ExternalEvent::Submit(UserPrompt {
                text: "hi".into(),
                generation: None,
            }),
        )
        .await
        .unwrap();

    assert_eq!(
        timeout(TIMEOUT, router.wait_idle(prime)).await.unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: None,
                error: Some("unknown assistant \"gone\"".into()),
            },
        })
    );
}

/// M1: a failed turn fires the wait typed — `error` carries the
/// failure and the cache keeps the last good output
#[tokio::test]
async fn failed_turn_fires_wait_typed_without_clobbering_output_cache() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "good").await;

    rig.api.script_turn(vec![AssistantEvent::Failed {
        message: "rate limited".into(),
        ended_at: 9,
    }]);
    assert_eq!(
        rig.router
            .send_message(rig.primary.clone(), child.clone(), "again".into())
            .await
            .unwrap(),
        Ok(())
    );

    let outcome = timeout(TIMEOUT, rig.router.wait(rig.primary.clone(), child))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome,
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("good".into()),
                error: Some("rate limited".into()),
            },
        })
    );
}

/// A send burst that fills the inter-agent mailbox surfaces `Busy` without
/// starving the separate user-control channel, and a live run loop drains a
/// burst back down into the target's history.
#[tokio::test]
async fn send_burst_fills_mailbox_without_starving_user_channel() {
    let mut rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "seed").await;

    // saturate the primary's test-held mailbox: nothing drains it
    let mut sent = 0;
    loop {
        let outcome = rig
            .router
            .send_message(child.clone(), rig.primary.clone(), format!("m{sent}"))
            .await
            .unwrap();
        match outcome {
            Ok(()) => sent += 1,
            Err(RouterError::Busy) => break,
            other => panic!("{other:?}"),
        }
        assert!(sent <= 64, "mailbox never filled");
    }

    // the user channel is a separate lane: control still lands while full
    rig.router
        .forward(
            rig.primary.clone(),
            ExternalEvent::Submit(UserPrompt {
                text: "user".into(),
                generation: None,
            }),
        )
        .await
        .unwrap();
    rig.router
        .forward(rig.primary.clone(), ExternalEvent::Abort)
        .await
        .unwrap();
    let first = timeout(TIMEOUT, rig._user_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(first, AgentEvent::External(ExternalEvent::Submit(_))),
        "{first:?}"
    );
    let second = timeout(TIMEOUT, rig._user_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(second, AgentEvent::External(ExternalEvent::Abort)),
        "{second:?}"
    );

    // a real run loop drains a burst: the first message wakes a turn, the
    // rest buffer and flush on idle — how they split into waves is timing-
    // dependent, so script generously and assert every message reaches the
    // child's persisted history
    for i in 0..4 {
        script_turn(&rig.api, &format!("d{i}"), "drained");
    }
    for i in 1..=10 {
        let outcome = rig
            .router
            .send_message(rig.primary.clone(), child.clone(), format!("b{i}"))
            .await
            .unwrap();
        assert_eq!(outcome, Ok(()));
    }
    timeout(TIMEOUT, async {
        loop {
            let state = rig.project.store().load_state(&child).await.unwrap();
            let texts: Vec<String> = state
                .context
                .history
                .state()
                .iter()
                .filter_map(|m| match m {
                    crate::llm::history::message::Message::User(u) => Some(u.text.clone()),
                    _ => None,
                })
                .collect();
            if (1..=10).all(|i| texts.iter().any(|t| t.ends_with(&format!("\nb{i}")))) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("burst never drained into the child's history");
}

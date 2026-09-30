//! router unit tests: through the real router,
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
use tokio::time::timeout;

use super::api::RouterError;
use super::api::TurnOutcome;
use super::api::WaitResult;
use super::graph::NodeStatus;
use super::*;
use crate::agent::Agent;
use crate::agent::event::AgentEvent;
use crate::agent::event::UserCommand;
use crate::agent::event::UserPrompt;
use crate::llm::history::AssistantEvent;
use crate::llm::history::delta::Delta;
use crate::llm::history::delta::DeltaContent;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::OutputItem;
use crate::llm::provider::api::fake::FakeApi;

const TIMEOUT: Duration = Duration::from_secs(5);

impl Router {
    /// a real router nobody drives — for tests that instantiate Agents or
    /// tabs without exercising the graph
    pub fn test_handle(project: &Project) -> Self {
        let (app_tx, app_rx) = channel(1);
        std::mem::forget(app_rx);
        RouterState::start(
            app_tx,
            project.clone(),
            Default::default(),
            Default::default(),
            Default::default(),
        )
    }

    /// register a root whose channels the test drives by hand — no runtime
    /// task, so nothing reports status on its own
    pub fn attach_manual(
        &self,
        aid: &AgentId,
        tx: Sender<AgentEvent>,
        user_tx: Sender<AgentEvent>,
    ) {
        let (abort, _registration) = AbortHandle::new_pair();
        self.register_root(aid).unwrap();
        self.lock().attach(aid, tx, user_tx, abort).unwrap();
    }

    /// Abort the agent's live runtime; the node stays reachable and the
    /// persisted state and workdir are untouched.
    pub fn shutdown(
        &self,
        aid: &AgentId,
    ) -> anyhow::Result<()> {
        let s = &mut *self.lock();
        let abort = s.graph.get(aid).and_then(|n| n.abort.clone());
        let abort = abort.ok_or_else(|| anyhow::anyhow!("no live runtime for {aid}"))?;
        abort.abort();
        s.fail_runtime(aid, "agent runtime cancelled by test".into(), false);
        Ok(())
    }

    /// test-facing sync primitive: fire on the target's next idle/dead
    /// transition — the same signal `wait` consumes, minus root/cycle checks
    pub async fn wait_idle(
        &self,
        aid: &AgentId,
    ) -> Result<WaitResult, RouterError> {
        let rx = {
            let s = &mut *self.lock();
            match s.graph.get(aid) {
                None => return Err(RouterError::Unreachable),
                Some(node) if matches!(node.status, NodeStatus::Idle | NodeStatus::Dead) => {
                    return Ok(node.wait_result());
                }
                // self-edge: adds nothing reachable to the wait-for graph,
                // so `would_deadlock` never sees it
                Some(_) => s.register_waiter(aid, aid),
            }
        };
        rx.await.expect("waiter dropped unfired")
    }
}

struct Rig {
    project: Project,
    api: Arc<FakeApi>,
    router: Router,
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
        let child = self.router.spawn_agent(&parent, None, "go").await.unwrap();
        let outcome = timeout(TIMEOUT, self.router.wait(&parent, &child))
            .await
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

/// an app bus nobody renders: drained so agent emits never block
fn drained_app_tx() -> Sender<AppEvent> {
    let (app_tx, mut app_rx) = channel(256);
    tokio::spawn(async move { while app_rx.recv().await.is_some() {} });
    app_tx
}

fn spawn_router(
    project: &Project,
    records: BTreeMap<AgentId, graph::GraphRecord>,
) -> Router {
    let outcomes = records
        .keys()
        .map(|a| (a.clone(), Default::default()))
        .collect();
    RouterState::start(
        drained_app_tx(),
        project.clone(),
        records,
        Default::default(),
        outcomes,
    )
}

/// a primary with a workdir + saved state and a dummy (test-held) runtime
async fn register_primary(
    project: &Project,
    router: &Router,
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
    router.attach_manual(&aid, tx, user_tx);
    router.status(
        &aid,
        graph::StatusReport {
            processed: 0,
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: None,
                error: None,
            },
        },
    );
    (aid, rx, user_rx)
}

async fn start_saved_agents(
    project: &Project,
    router: &Router,
    aids: &[AgentId],
) {
    let mut agents = Vec::new();
    for aid in aids {
        let state = project.store().load_state(aid).await.unwrap();
        agents.push(Agent::new(
            project.clone(),
            router.clone(),
            drained_app_tx(),
            aid.clone(),
            state,
        ));
    }
    router.launch(agents).unwrap();
    for aid in aids {
        assert!(matches!(
            timeout(TIMEOUT, router.wait_idle(&aid)).await.unwrap(),
            Ok(WaitResult { .. })
        ));
    }
}

#[test]
fn free_variant_suffixes_per_name() {
    let mut taken = BTreeSet::new();
    assert_eq!(ops::free_variant("a-b-c", &taken).to_string(), "a-b-c");
    taken.insert(AgentId::from("a-b-c".to_string()));
    assert_eq!(ops::free_variant("a-b-c", &taken).to_string(), "a-b-c-2");
    taken.insert(AgentId::from("a-b-c-2".to_string()));
    assert_eq!(ops::free_variant("a-b-c", &taken).to_string(), "a-b-c-3");
}

#[tokio::test]
async fn spawn_registers_under_parent_and_wait_collects_output() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "child result").await;

    let members = rig.router.list(&rig.primary, false).unwrap();
    let child_member = members.iter().find(|m| m.id == child).unwrap();
    assert_eq!(child_member.parent, Some(rig.primary.clone()));
    assert_eq!(child_member.status, NodeStatus::Idle);

    // a second wait on the already-idle target fires immediately
    let outcome = rig.router.wait(&rig.primary, &child).await;
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
        rig.router.inspect(&rig.primary, &child),
        Ok(NodeStatus::Idle)
    );
}

#[tokio::test]
async fn send_bumps_delivered_so_racing_wait_returns_post_message_output() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "first").await;

    rig.script("second", "second");
    let sent = rig.router.send_message(&rig.primary, &child, "more");
    assert_eq!(sent, Ok(()));
    // the delivery is outstanding: this wait must register, not fire on the
    // pre-message idle
    let outcome = timeout(TIMEOUT, rig.router.wait(&rig.primary, &child))
        .await
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
    // one poll registers the child under the lock and leaves its setup
    // tail in flight, so we see it still spawning
    let mut spawn = std::pin::pin!(rig.router.spawn_agent(&rig.primary, None, "go"));
    assert!(futures::poll!(&mut spawn).is_pending());
    let members = rig.router.list(&rig.primary, true).unwrap();
    let child = members.iter().find(|m| m.id != rig.primary).unwrap();
    assert_eq!(child.status, NodeStatus::Spawning);

    // parks behind the seed prompt — never Unreachable, never blocking
    // (busy-buffering of the parked message itself lands with step 3)
    let sent = rig.router.send_message(&rig.primary, &child.id, "psst");
    assert_eq!(sent, Ok(()));
    timeout(TIMEOUT, spawn).await.unwrap().unwrap();
}

#[tokio::test]
async fn cross_tab_everything_is_unreachable() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "mine").await;
    let (other, _rx, _user_rx) = register_primary(&rig.project, &rig.router, "other").await;

    assert_eq!(
        rig.router.send_message(&other, &child, "hi"),
        Err(RouterError::Unreachable)
    );
    assert_eq!(
        rig.router.inspect(&other, &child),
        Err(RouterError::Unreachable)
    );
    assert_eq!(
        rig.router.wait(&other, &child).await,
        Err(RouterError::Unreachable)
    );
    assert_eq!(
        rig.router.archive(&other, &child).await.unwrap(),
        Err(RouterError::Unreachable)
    );
    // list shows exactly the caller's tab
    let members = rig.router.list(&other, false).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].id, other);
    // unknown caller
    let ghost = AgentId::from("nobody".to_string());
    assert_eq!(
        rig.router.send_message(&ghost, &child, "hi"),
        Err(RouterError::Unreachable)
    );
    assert_eq!(rig.router.list(&ghost, false), None);
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
            rig.router.archive(&caller, &target).await.unwrap(),
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
        rig.router.archive(&rig.primary, &child).await.unwrap(),
        Ok(())
    );

    // exact subtree unreachable, sibling untouched
    for aid in [&child, &grandchild] {
        assert_eq!(
            rig.router.send_message(&rig.primary, &aid, "hi"),
            Err(RouterError::Unreachable)
        );
    }
    let members = rig.router.list(&rig.primary, false).unwrap();
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

    rig.router.archive_tab(&rig.primary).await.unwrap();

    assert_eq!(rig.router.list(&rig.primary, false), None);
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
        .archive(&rig.primary, &archived)
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
        router2.wait(&rig.primary, &kept).await,
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("resumed".into()),
                error: None,
            },
        })
    );
    assert_eq!(router2.inspect(&rig.primary, &kept), Ok(NodeStatus::Idle));
    assert_eq!(
        router2.send_message(&rig.primary, &archived, "hi"),
        Err(RouterError::Unreachable)
    );
    assert_eq!(
        router2.wait(&rig.primary, &archived).await,
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
    let router = RouterState::start(
        drained_app_tx(),
        project.clone(),
        records,
        Default::default(),
        outcomes,
    );
    start_saved_agents(&project, &router, &[prime.clone(), descendant.clone()]).await;

    assert_eq!(
        router.wait(&prime, &bad).await,
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: None,
                error: Some("corrupt child row".into()),
            },
        })
    );
    assert_eq!(router.inspect(&prime, &descendant), Ok(NodeStatus::Idle));
}

#[tokio::test]
async fn runtime_death_is_terminal_until_restart() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "answer").await;

    rig.router.shutdown(&child).unwrap();

    assert_eq!(
        rig.router.wait(&rig.primary, &child).await,
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: Some("answer".into()),
                error: Some("agent runtime cancelled by test".into()),
            },
        })
    );
    assert_eq!(
        rig.router.send_message(&rig.primary, &child, "hi"),
        Err(RouterError::Unreachable)
    );
    assert!(rig.router.forward(&child, UserCommand::Abort).is_err());
    let err = rig
        .router
        .spawn_agent(&child, None, "go")
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), format!("agent {child} is dead"));
    // A late status report cannot revive or overwrite a terminal node.
    rig.router.status(
        &child,
        graph::StatusReport {
            processed: 99,
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: Some("ghost".into()),
                error: None,
            },
        },
    );
    assert_eq!(
        rig.router.wait(&rig.primary, &child).await,
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
        router2.wait(&rig.primary, &child).await,
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
        .spawn_agent(&prime, None, "go")
        .await
        .unwrap_err()
        .to_string();
    // recovery is spelled out — ids may be gone from compacted history
    assert!(err.contains("archive") && err.contains("list"), "{err}");

    // archive frees the slot; the next spawn goes through
    assert_eq!(
        router
            .archive(&prime, &AgentId::from("m1".to_string()))
            .await
            .unwrap(),
        Ok(())
    );
    script_turn(&api, "o1", "fits now");
    let child = router.spawn_agent(&prime, None, "go").await.unwrap();
    assert_eq!(
        timeout(TIMEOUT, router.wait(&prime, &child)).await.unwrap(),
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
    // outstanding delivery alone makes a racing wait register
    rig.api.script_hanging_turn(vec![]);
    rig.api.script_hanging_turn(vec![]);
    for target in [&a, &b] {
        assert_eq!(
            rig.router.send_message(&rig.primary, &target, "work"),
            Ok(())
        );
    }

    // a → b edge, receiver kept alive
    let ab_rx = rig.router.lock().register_waiter(&a, &b);
    // closing the cycle is rejected, typed
    assert_eq!(
        rig.router.wait(&b, &a).await,
        Err(RouterError::WouldDeadlock)
    );

    // dropped registration = stale edge: pruned, the reverse wait registers
    drop(ab_rx);
    let reverse = tokio::spawn({
        let router = rig.router.clone();
        let (a, b) = (a.clone(), b.clone());
        async move { router.wait(&b, &a).await }
    });
    // the abort rides the priority user channel: let a's hanging turn start
    // first, or the abort lands before the wake and aborts nothing — a
    // delivery alone already reads Running, so wait for a to process it
    timeout(TIMEOUT, async {
        loop {
            let started = {
                let s = rig.router.lock();
                let node = &s.graph[&a];
                node.processed == node.delivered && node.status == NodeStatus::Running
            };
            if started {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a never started its hanging turn");
    // abort a's hanging turn: it idles with the typed failure, the cached
    // output intact
    rig.router.forward(&a, UserCommand::Abort).unwrap();
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
        rig.router.spawn_agent(&rig.primary, None, "go"),
        rig.router.spawn_agent(&rig.primary, None, "go"),
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

    let result = rig.router.spawn_agent(&rig.primary, None, "go").await;
    assert!(result.is_err());

    let members = rig.router.list(&rig.primary, true).unwrap();
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
    state.assistant_id = "ghost".into();
    rig.project
        .store()
        .save_state(&rig.primary, &state)
        .await
        .unwrap();

    let result = rig.router.spawn_agent(&rig.primary, None, "go").await;
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
    let mut spawn = std::pin::pin!(rig.router.spawn_agent(&rig.primary, None, "go"));
    assert!(futures::poll!(&mut spawn).is_pending());
    let members = rig.router.list(&rig.primary, true).unwrap();
    let child = members
        .iter()
        .find(|m| m.id != rig.primary)
        .unwrap()
        .id
        .clone();

    // archive while the spawn tail is (likely) still in flight
    assert_eq!(
        rig.router.archive(&rig.primary, &child).await.unwrap(),
        Ok(())
    );
    // Let the tail land either way. If archive won before attachment, setup
    // rolls back; if attachment won, the durable archived graph record wins.
    drop(timeout(TIMEOUT, spawn).await);

    let records = rig.project.store().load_graph().await.unwrap();
    assert!(records.get(&child).is_none_or(|record| record.archived));
    assert_eq!(
        rig.router.send_message(&rig.primary, &child, "hi"),
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
    router.register_root(&aid).unwrap();
    router.lock().attach(&aid, tx1, user_tx1, abort1).unwrap();

    let (tx2, _rx2) = channel(8);
    let (user_tx2, _user_rx2) = channel(8);
    let (abort2, _reg2) = AbortHandle::new_pair();
    assert!(router.lock().attach(&aid, tx2, user_tx2, abort2).is_err());

    // Success is returned only after the event is actually queued.
    router.forward(&aid, UserCommand::Abort).unwrap();
    assert!(router.forward(&aid, UserCommand::Abort).is_err());
    let event = timeout(TIMEOUT, user_rx1.recv()).await.unwrap().unwrap();
    assert!(matches!(event, AgentEvent::User(UserCommand::Abort)));
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
                &aid,
                UserCommand::Submit(UserPrompt {
                    text: "hello again".into(),
                    generation: None,
                })
            )
            .is_err()
    );

    assert_eq!(
        router.wait_idle(&aid).await,
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: None,
                error: Some("agent runtime mailbox closed".into()),
            },
        })
    );
}

/// a send that lands while the target's pre-send idle report is still in
/// flight must not fire a later wait with the previous turn's output — the
/// delivery seqnums swallow the stale report
#[tokio::test]
async fn wait_after_mid_idle_send_returns_post_message_output() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "seed").await;
    let report = |processed, status, output: Option<&str>| graph::StatusReport {
        processed,
        status,
        outcome: TurnOutcome {
            output: output.map(Into::into),
            error: None,
        },
    };

    // the primary's (emulated) turn ends; its idle report is still in flight…
    rig.router
        .status(&rig.primary, report(0, NodeStatus::Running, None));
    // …when the child's message is delivered
    assert_eq!(
        rig.router.send_message(&child, &rig.primary, "more"),
        Ok(())
    );
    // the stale pre-delivery idle report arrives: processed < delivered, so a
    // wait registered now must not fire on "old"
    rig.router
        .status(&rig.primary, report(0, NodeStatus::Idle, Some("old")));
    let wait_rx = rig.router.lock().register_waiter(&child, &rig.primary);
    // the agent catches up: processes the delivery, runs its turn to idle
    rig.router
        .status(&rig.primary, report(1, NodeStatus::Running, None));
    rig.router
        .status(&rig.primary, report(1, NodeStatus::Idle, Some("new")));
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

    router.runtime_down(&aid, "first failure".into());
    router.runtime_down(&aid, "second failure".into());

    assert_eq!(
        timeout(TIMEOUT, router.wait_idle(&aid)).await.unwrap(),
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
    rig.router.shutdown(&rig.primary).unwrap();

    assert!(
        rig.router
            .forward(&rig.primary, UserCommand::Abort)
            .is_err()
    );

    assert_eq!(
        timeout(TIMEOUT, rig.router.wait_idle(&rig.primary))
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

/// a wake whose turn fails to start still reports — the wait fires with
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
    state.assistant_id = "gone".into();
    project.store().save_state(&prime, &state).await.unwrap();
    start_saved_agents(&project, &router, std::slice::from_ref(&prime)).await;

    router
        .forward(
            &prime,
            UserCommand::Submit(UserPrompt {
                text: "hi".into(),
                generation: None,
            }),
        )
        .unwrap();

    assert_eq!(
        timeout(TIMEOUT, router.wait_idle(&prime)).await.unwrap(),
        Ok(WaitResult {
            status: NodeStatus::Idle,
            outcome: TurnOutcome {
                output: None,
                error: Some("unknown assistant \"gone\"".into()),
            },
        })
    );
}

/// a failed turn fires the wait typed — `error` carries the
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
        rig.router.send_message(&rig.primary, &child, "again"),
        Ok(())
    );

    let outcome = timeout(TIMEOUT, rig.router.wait(&rig.primary, &child))
        .await
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
            .send_message(&child, &rig.primary, &format!("m{sent}"));
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
            &rig.primary,
            UserCommand::Submit(UserPrompt {
                text: "user".into(),
                generation: None,
            }),
        )
        .unwrap();
    rig.router
        .forward(&rig.primary, UserCommand::Abort)
        .unwrap();
    let first = timeout(TIMEOUT, rig._user_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(first, AgentEvent::User(UserCommand::Submit(_))),
        "{first:?}"
    );
    let second = timeout(TIMEOUT, rig._user_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(second, AgentEvent::User(UserCommand::Abort)),
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
            .send_message(&rig.primary, &child, &format!("b{i}"));
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

async fn supervised(
    future: impl std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    cancel: bool,
) -> Result<WaitResult, RouterError> {
    let project = Project::new_test().unwrap().0;
    let (app_tx, _app_rx) = channel(8);
    let router = RouterState::start(
        app_tx,
        project,
        Default::default(),
        Default::default(),
        Default::default(),
    );
    let aid = AgentId::from(format!("supervised-{}", uuid::Uuid::new_v4()));
    router.register_root(&aid).unwrap();
    let (tx, _rx) = channel(1);
    let (user_tx, _user_rx) = channel(1);
    let (abort, registration) = AbortHandle::new_pair();
    router
        .lock()
        .attach(&aid, tx, user_tx, abort.clone())
        .unwrap();
    tokio::spawn(ops::runtime::supervise(
        aid.clone(),
        router.clone(),
        future,
        registration,
    ));
    if cancel {
        abort.abort();
    }
    timeout(Duration::from_secs(1), router.wait_idle(&aid))
        .await
        .unwrap()
}

#[tokio::test]
async fn supervisor_reports_return_error_panic_and_cancellation() {
    assert_eq!(
        supervised(async { Ok(()) }, false).await,
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: None,
                error: Some("agent runtime exited unexpectedly".into()),
            },
        })
    );
    assert_eq!(
        supervised(async { Err(anyhow::anyhow!("fatal")) }, false).await,
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: None,
                error: Some("agent runtime failed: fatal".into()),
            },
        })
    );
    assert_eq!(
        supervised(
            async {
                panic!("boom");
                #[allow(unreachable_code)]
                Ok(())
            },
            false,
        )
        .await,
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: None,
                error: Some("agent runtime panicked: boom".into()),
            },
        })
    );
    assert_eq!(
        supervised(futures::future::pending(), true).await,
        Ok(WaitResult {
            status: NodeStatus::Dead,
            outcome: TurnOutcome {
                output: None,
                error: Some("agent runtime cancelled unexpectedly".into()),
            },
        })
    );
}

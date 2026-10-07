//! router unit tests: through the real router,
//! with real child runtimes on scripted `FakeApi` turns
#![cfg(test)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use futures::future::AbortHandle;
use similar_asserts::assert_eq;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::mpsc::unbounded_channel;
use tokio::time::timeout;

use super::api::RouterError;
use super::graph::NodeStatus;
use super::graph::Runtime;
use super::*;
use crate::agent::event::AgentEvent;
use crate::llm::history::AssistantEvent;
use crate::llm::history::delta::Delta;
use crate::llm::history::delta::DeltaContent;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::Message;
use crate::llm::history::message::OutputItem;
use crate::llm::provider::api::fake::FakeApi;

const TIMEOUT: Duration = Duration::from_secs(5);

impl Router {
    /// a live, durable root whose mailbox the test drives by hand — no
    /// runtime task, so nothing reports status on its own
    pub fn attach_manual(
        &self,
        aid: &AgentId,
        mailbox: UnboundedSender<AgentEvent>,
    ) {
        let (abort, _registration) = AbortHandle::new_pair();
        let node = AgentNode::live(aid.clone(), None, mailbox, abort);
        let s = &mut *self.lock();
        drop(s.project.store().save_graph(aid, &node.record(false)));
        s.graph.insert(aid.clone(), node);
    }

    /// Abort the agent's live runtime; the node stays reachable and the
    /// persisted state and workdir are untouched.
    pub fn shutdown(
        &self,
        aid: &AgentId,
    ) -> anyhow::Result<()> {
        // one lock: the supervisor's own report of the abort lands after
        // ours, on a node that's already dead
        let s = &mut *self.lock();
        let Some(node) = s.graph.get_mut(aid) else {
            anyhow::bail!("unknown agent {aid}");
        };
        let Runtime::Live { abort, .. } = &node.runtime else {
            anyhow::bail!("no live runtime for {aid}");
        };
        abort.abort();
        node.runtime = Runtime::Dead("agent runtime cancelled by test".into());
        Ok(())
    }

    fn status(
        &self,
        aid: &AgentId,
    ) -> Option<NodeStatus> {
        self.lock().graph.get(aid).map(AgentNode::status)
    }

    /// test-facing sync point: poll until `aid` is idle with an assistant
    /// message reading `text` in its saved history. The output is saved
    /// while the turn still runs, so idle-after-output is the turn's end,
    /// never the idle before the mail was handled
    pub async fn idle_with_output(
        &self,
        aid: &AgentId,
        text: &str,
    ) {
        let project = self.lock().project.clone();
        timeout(TIMEOUT, async {
            loop {
                let answered = project.store().load_state(aid).await.is_ok_and(|state| {
                    state
                        .context
                        .history
                        .state()
                        .iter()
                        .filter_map(|m| m.try_as_assistant_ref())
                        .any(|m| m.text_output() == text)
                });
                if answered && self.status(aid) == Some(NodeStatus::Idle) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{aid} never went idle on {text:?}"));
    }

    /// poll until `aid` is dead; returns what a send to it reports
    pub async fn death(
        &self,
        aid: &AgentId,
    ) -> Result<(), RouterError> {
        timeout(TIMEOUT, async {
            while self.status(aid) != Some(NodeStatus::Dead) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{aid} never died"));
        self.send_message(aid, aid, "ping")
    }
}

struct Rig {
    project: Project,
    api: Arc<FakeApi>,
    router: Router,
    primary: AgentId,
    /// the dummy primary's mailbox: nobody runs it
    primary_mail: UnboundedReceiver<AgentEvent>,
    /// the repo's HEAD: the tab's snapshot, and where children start
    commit: String,
}

impl Rig {
    async fn new(name: &str) -> Self {
        let (project, api) = Project::new_test().unwrap();
        let router = Router::new(unbounded_channel().0, project.clone());
        let (primary, primary_mail) = register_primary(&project, &router, name).await;
        let commit = project.head_commit();
        Self {
            project,
            api,
            router,
            primary,
            primary_mail,
            commit,
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
            .spawn_agent(&parent, &self.commit, None, "go")
            .await
            .unwrap();
        self.router.idle_with_output(&child, text).await;
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

/// a primary with a workdir + saved state and a dummy (test-held) runtime,
/// reported idle
async fn register_primary(
    project: &Project,
    router: &Router,
    name: &str,
) -> (AgentId, UnboundedReceiver<AgentEvent>) {
    let aid = AgentId::from(name.to_string());
    tokio::fs::create_dir_all(project.agent_workdir(&aid))
        .await
        .unwrap();
    project
        .store()
        .save_state(&aid, &project.fake_state())
        .await
        .unwrap();
    let (mailbox, mail) = unbounded_channel();
    router.attach_manual(&aid, mailbox);
    router.report_status(&aid, NodeStatus::Idle);
    (aid, mail)
}

/// boot from the store as the app does, and start every restored agent
async fn reboot(project: &Project) -> Router {
    start(
        Router::boot(unbounded_channel().0, project.clone())
            .await
            .unwrap(),
    )
    .await
}

/// launch every restored agent; up once the runtimes settle idle
async fn start(boot: boot::Boot) -> Router {
    let aids: Vec<AgentId> = boot.agents.iter().map(|l| l.agent.id.clone()).collect();
    for launch in boot.agents {
        launch.go();
    }
    timeout(TIMEOUT, async {
        while aids
            .iter()
            .any(|aid| boot.router.status(aid) == Some(NodeStatus::Running))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("restored agents never started");
    boot.router
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
async fn spawn_registers_under_parent() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "child result").await;

    let members = rig.router.list(&rig.primary, false).unwrap();
    let child_member = members.iter().find(|m| m.id == child).unwrap();
    assert_eq!(child_member.parent, Some(rig.primary.clone()));
    assert_eq!(child_member.status, NodeStatus::Idle);
}

#[tokio::test]
async fn send_wakes_an_idle_target() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "first").await;

    rig.script("second", "second");
    let sent = rig.router.send_message(&rig.primary, &child, "more");
    assert_eq!(sent, Ok(()));
    rig.router.idle_with_output(&child, "second").await;
}

/// a child reports back the only way there is: a send into the parent's
/// mailbox, stamped with the sender
#[tokio::test]
async fn child_reports_to_parent_by_send() {
    let mut rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "seed").await;

    assert_eq!(
        rig.router.send_message(&child, &rig.primary, "done"),
        Ok(())
    );
    let Some(AgentEvent::Message(report)) =
        timeout(TIMEOUT, rig.primary_mail.recv()).await.unwrap()
    else {
        panic!("no report");
    };
    assert_eq!(report.text, format!("[from: {child}]\ndone"));
}

/// a spawn's node joins the graph only once set up: nobody sees it in
/// progress
#[tokio::test]
async fn spawn_in_progress_is_invisible() {
    let rig = Rig::new("prime").await;
    rig.script("o1", "seeded");
    // one poll leaves the setup tail in flight
    let mut spawn = std::pin::pin!(
        rig.router
            .spawn_agent(&rig.primary, &rig.commit, None, "go")
    );
    assert!(futures::poll!(&mut spawn).is_pending());
    let members = rig.router.list(&rig.primary, true).unwrap();
    assert_eq!(members.len(), 1);

    let child = timeout(TIMEOUT, spawn).await.unwrap().unwrap();
    rig.router.idle_with_output(&child, "seeded").await;
}

#[tokio::test]
async fn cross_tab_everything_is_unreachable() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "mine").await;
    let (other, _other_mail) = register_primary(&rig.project, &rig.router, "other").await;

    assert_eq!(
        rig.router.send_message(&other, &child, "hi"),
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
        .push(crate::llm::history::message::PeerMessage::new("prime", "resume", 10).into());
    rig.project
        .store()
        .save_state(&kept, &kept_state)
        .await
        .unwrap();
    rig.script("resumed", "resumed");

    let router2 = reboot(&rig.project).await;

    router2.idle_with_output(&kept, "resumed").await;
    assert_eq!(router2.status(&kept), Some(NodeStatus::Idle));
    assert_eq!(
        router2.send_message(&rig.primary, &archived, "hi"),
        Err(RouterError::Unreachable)
    );
    rig.project.store().load_state(&archived).await.unwrap();
    assert!(rig.project.agent_workdir(&archived).exists());
}

/// partial boot: archived records and an invalid root's whole tab stay out
/// (state under it is never read), an invalid child is a reachable `Dead`
/// node, and its valid descendant still starts
#[tokio::test]
async fn boot_isolates_invalid_agents() {
    let (project, _api) = Project::new_test().unwrap();
    let aid = |s: &str| AgentId::from(s.to_string());
    let record = |root: &str, parent: Option<&str>, archived| graph::GraphRecord {
        root: aid(root),
        parent: parent.map(aid),
        archived,
    };
    let records = [
        ("good", record("good", None, false)),
        ("bad-root", record("bad-root", None, false)),
        ("bad-child", record("good", Some("good"), false)),
        ("grandchild", record("good", Some("bad-child"), false)),
        (
            "under-bad-root",
            record("bad-root", Some("bad-root"), false),
        ),
        ("archived", record("archived", None, true)),
    ]
    .map(|(id, record)| (aid(id), record));
    project.store().save_graph_batch(&records).await.unwrap();
    // `bad-*` have no state to load
    for id in ["good", "grandchild", "under-bad-root", "archived"] {
        tokio::fs::create_dir_all(project.agent_workdir(&aid(id)))
            .await
            .unwrap();
        project
            .store()
            .save_state(&aid(id), &project.fake_state())
            .await
            .unwrap();
    }

    let boot = Router::boot(unbounded_channel().0, project.clone())
        .await
        .unwrap();
    let graph: BTreeMap<AgentId, NodeStatus> = boot
        .router
        .lock()
        .graph
        .iter()
        .map(|(id, node)| (id.clone(), node.status()))
        .collect();
    insta::assert_yaml_snapshot!(serde_json::json!({
        "tabs": boot.tabs.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        "agents": boot.agents.iter().map(|l| &l.agent.id).collect::<Vec<_>>(),
        "failures": boot.failures,
        "graph": graph,
        "all_ids": boot.router.lock().all_ids,
    }), @"
    agents:
      - good
      - grandchild
    all_ids:
      - archived
      - bad-child
      - bad-root
      - good
      - grandchild
      - under-bad-root
    failures:
      - - bad-root
        - agent bad-root not found
      - - bad-child
        - agent bad-child not found
    graph:
      bad-child: Dead
      good: Running
      grandchild: Running
    tabs:
      - good
    ");

    let router = start(boot).await;
    assert_eq!(router.status(&aid("grandchild")), Some(NodeStatus::Idle));
    assert_eq!(
        router.send_message(&aid("good"), &aid("bad-child"), "hi"),
        Err(RouterError::Dead("agent bad-child not found".into()))
    );
}

#[tokio::test]
async fn runtime_death_is_terminal_until_restart() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "answer").await;

    rig.router.shutdown(&child).unwrap();

    let dead = Err(RouterError::Dead("agent runtime cancelled by test".into()));
    assert_eq!(rig.router.send_message(&rig.primary, &child, "hi"), dead);
    let err = rig
        .router
        .spawn_agent(&child, &rig.commit, None, "go")
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), format!("agent {child} is dead"));
    // A late status report cannot revive or overwrite a terminal node.
    rig.router.report_status(&child, NodeStatus::Idle);
    assert_eq!(rig.router.send_message(&rig.primary, &child, "hi"), dead);
    let records = rig.project.store().load_graph().await.unwrap();
    assert!(!records[&child].archived);

    let router2 = reboot(&rig.project).await;
    assert_eq!(router2.status(&child), Some(NodeStatus::Idle));
}

#[tokio::test]
async fn spawn_errors_at_the_tab_cap_and_archive_frees_a_slot() {
    let (project, api) = Project::new_test().unwrap();
    let prime = AgentId::from("prime".to_string());
    // Boot with the tab already at the cap. Durable members hold slots
    // whatever their runtime: these have no state, so they boot `Dead`.
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
    .collect::<Vec<_>>();
    project.store().save_graph_batch(&records).await.unwrap();
    tokio::fs::create_dir_all(project.agent_workdir(&prime))
        .await
        .unwrap();
    project
        .store()
        .save_state(&prime, &project.fake_state())
        .await
        .unwrap();
    let router = reboot(&project).await;

    let err = router
        .spawn_agent(&prime, &project.head_commit(), None, "go")
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
    let child = router
        .spawn_agent(&prime, &project.head_commit(), None, "go")
        .await
        .unwrap();
    router.idle_with_output(&child, "fits now").await;
}

/// a spawn checks the child out at the requested commit, on its own
/// branch over the tab's snapshot; the spawner's uncommitted work stays out
#[tokio::test]
async fn spawn_checks_the_child_out_at_the_requested_commit() {
    let rig = Rig::new("prime").await;
    std::fs::write(
        rig.project.agent_workdir(&rig.primary).join("draft.txt"),
        "uncommitted\n",
    )
    .unwrap();
    let repo = git2::Repository::open(rig.project.root()).unwrap();
    let start = {
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let mut tree = repo.treebuilder(Some(&head.tree().unwrap())).unwrap();
        let blob = repo.blob(b"committed\n").unwrap();
        tree.insert("work.txt", blob, 0o100_644).unwrap();
        let tree = repo.find_tree(tree.write().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        repo.commit(None, &sig, &sig, "work", &tree, &[&head])
            .unwrap()
            .to_string()
    };
    rig.script("done", "done");
    let child = rig
        .router
        .spawn_agent(&rig.primary, &start, None, "go")
        .await
        .unwrap();
    rig.router.idle_with_output(&child, "done").await;

    let workdir = rig.project.agent_workdir(&child);
    assert_eq!(
        std::fs::read_to_string(workdir.join("work.txt")).unwrap(),
        "committed\n"
    );
    assert!(!workdir.join("draft.txt").exists());
    let branch = repo
        .find_branch(&rig.project.worktree_name(&child), git2::BranchType::Local)
        .unwrap();
    assert_eq!(branch.get().target().unwrap().to_string(), start);
    // the snapshot stays the tab's
    let state = rig.project.store().load_state(&child).await.unwrap();
    assert_eq!(state.context.commit, rig.commit);
}

#[tokio::test]
async fn failed_spawn_rolls_back_node_row_record_and_workdir() {
    let rig = Rig::new("prime").await;
    // nothing to check out → the detached tail fails after registration
    let missing = "0".repeat(39) + "1";

    let result = rig
        .router
        .spawn_agent(&rig.primary, &missing, None, "go")
        .await;
    assert!(result.is_err());

    let members = rig.router.list(&rig.primary, true).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].id, rig.primary);
    let records = rig.project.store().load_graph().await.unwrap();
    assert_eq!(records.len(), 1);
    assert!(!records[&rig.primary].archived);
    assert_eq!(rig.project.store().load_graph().await.unwrap().len(), 1);
}

/// a spawn failing *after* the checkout (bad assistant id) rolls the branch
/// back with the state, graph record and dir — no `vc-*` residue
#[tokio::test]
async fn failed_spawn_after_checkout_leaves_no_branch() {
    let rig = Rig::new("prime").await;
    let mut state = rig.project.fake_state();
    state.assistant_id = "ghost".into();
    rig.project
        .store()
        .save_state(&rig.primary, &state)
        .await
        .unwrap();

    let result = rig
        .router
        .spawn_agent(&rig.primary, &rig.commit, None, "go")
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
    let branches: Vec<String> = repo
        .branches(Some(git2::BranchType::Local))
        .unwrap()
        .filter_map(|b| b.unwrap().0.name().unwrap().map(str::to_string))
        .filter(|name| name.starts_with("vc-"))
        .collect();
    assert_eq!(branches, Vec::<String>::new());
}

/// the parent archived mid-spawn: the commit finds it gone and rolls the
/// child back, so it never joins the graph
#[tokio::test]
async fn spawn_under_parent_archived_mid_setup_rolls_back() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "child").await;
    let mut spawn = std::pin::pin!(rig.router.spawn_agent(&child, &rig.commit, None, "go"));
    assert!(futures::poll!(&mut spawn).is_pending());

    assert_eq!(
        rig.router.archive(&rig.primary, &child).await.unwrap(),
        Ok(())
    );
    let err = timeout(TIMEOUT, spawn).await.unwrap().unwrap_err();
    assert_eq!(err.to_string(), format!("unknown agent {child}"));

    let records = rig.project.store().load_graph().await.unwrap();
    let ids: BTreeSet<&AgentId> = records.keys().collect();
    assert_eq!(ids, BTreeSet::from([&rig.primary, &child]));
    let dirs: BTreeSet<_> = std::fs::read_dir(rig.project.agents())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        dirs,
        BTreeSet::from([rig.primary.to_string().into(), child.to_string().into()])
    );
}

/// a closed mailbox means the runtime is gone: the send fails, but the
/// death is left to the supervisor, whose reason is the one that sticks
#[tokio::test]
async fn send_to_closed_mailbox_is_rejected_without_recording_a_death() {
    let (project, _api) = Project::new_test().unwrap();
    let router = Router::new(unbounded_channel().0, project.clone());
    let (aid, mail) = register_primary(&project, &router, "prime").await;
    drop(mail); // the runtime is gone, its supervisor hasn't reported yet

    assert_eq!(
        router.send_message(&aid, &aid, "hello again"),
        Err(RouterError::Dead("agent runtime mailbox closed".into()))
    );
    router.runtime_down(&aid, "agent runtime panicked: boom".into());
    assert_eq!(
        router.send_message(&aid, &aid, "and again"),
        Err(RouterError::Dead("agent runtime panicked: boom".into()))
    );
}

#[tokio::test]
async fn duplicate_runtime_down_keeps_the_first_terminal_error() {
    let (project, _api) = Project::new_test().unwrap();
    let router = Router::new(unbounded_channel().0, project.clone());
    let (aid, _mail) = register_primary(&project, &router, "prime").await;

    router.runtime_down(&aid, "first failure".into());
    router.runtime_down(&aid, "second failure".into());

    assert_eq!(
        router.send_message(&aid, &aid, "hi"),
        Err(RouterError::Dead("first failure".into()))
    );
}

/// the mailbox never pushes back: a live run loop drains a burst into the
/// target's history
#[tokio::test]
async fn send_burst_drains_into_the_target_history() {
    let rig = Rig::new("prime").await;
    let child = rig.spawn_idle_child(&rig.primary, "seed").await;

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
                    Message::Developer(DeveloperMessage::Peer(p)) => Some(p.text.clone()),
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
) -> Result<(), RouterError> {
    let project = Project::new_test().unwrap().0;
    let (app_tx, _app_rx) = unbounded_channel();
    let router = Router::new(app_tx, project);
    let aid = AgentId::from(format!("supervised-{}", uuid::Uuid::new_v4()));
    let (abort, registration) = AbortHandle::new_pair();
    let node = AgentNode::live(aid.clone(), None, unbounded_channel().0, abort.clone());
    router.lock().graph.insert(aid.clone(), node);
    tokio::spawn(ops::runtime::supervise(
        aid.clone(),
        router.clone(),
        future,
        registration,
    ));
    if cancel {
        abort.abort();
    }
    router.death(&aid).await
}

#[tokio::test]
async fn supervisor_reports_return_error_panic_and_cancellation() {
    let dead = |error: &str| Err(RouterError::Dead(error.into()));
    assert_eq!(
        supervised(async { Ok(()) }, false).await,
        dead("agent runtime exited unexpectedly")
    );
    assert_eq!(
        supervised(async { Err(anyhow::anyhow!("fatal")) }, false).await,
        dead("agent runtime failed: fatal")
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
        dead("agent runtime panicked: boom")
    );
    assert_eq!(
        supervised(futures::future::pending(), true).await,
        dead("agent runtime cancelled unexpectedly")
    );
}

//! `spawn`: a provisional node in-loop, then a detached setup task that
//! captures the parent's workdir/state and attaches the child runtime; any
//! failure rolls back through `RollbackSpawn`

use anyhow::Result;
use tokio::sync::mpsc::channel;
use tokio::sync::oneshot;

use super::inbound;
use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::router::AgentRouter;
use crate::agent::router::AgentRouterHandle;
use crate::agent::router::CHANNEL_CAPACITY;
use crate::agent::router::TAB_AGENT_CAP;
use crate::agent::router::api::RouterError;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::NodeStatus;
use crate::llm::history::History;

impl AgentRouter {
    pub fn handle_spawn(
        &mut self,
        parent: AgentId,
        capture: History,
        prompt: String,
        done: oneshot::Sender<Result<AgentId>>,
    ) {
        let root = match self.graph.get(&parent) {
            Some(node) if node.status != NodeStatus::Dead => node.root.clone(),
            Some(_) => {
                drop(done.send(Err(anyhow::anyhow!("agent {parent} is dead"))));
                return;
            }
            None => {
                drop(done.send(Err(anyhow::anyhow!("unknown agent {parent}"))));
                return;
            }
        };
        // per-tab count cap (§4 step 5); recovery must not depend on
        // remembering ids — the error spells out the list → archive loop
        if self.graph.values().filter(|n| n.root == root).count() >= TAB_AGENT_CAP {
            drop(done.send(Err(anyhow::anyhow!(
                "this tab is at its agent cap ({TAB_AGENT_CAP}): archive finished agents to \
                 free slots (list shows every member and its status)"
            ))));
            return;
        }
        let aid = self.allocate();
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        // fresh channel: the seed can't fail, and nothing can precede it
        drop(tx.try_send(inbound(&parent, &prompt)));
        let mut node = AgentNode::new(root, Some(parent.clone()), NodeStatus::Spawning);
        node.mailbox = Some(tx.clone());
        node.delivered = 1; // the parked seed
        // submitted in-loop, FIFO-ordered ahead of the tail's state save (H1);
        // the tail awaits this write so a failed graph write aborts the
        // spawn instead of leaving state without a graph record (M6)
        let record_write = self.project.store().save_graph(&aid, &node.record(false));
        self.graph.insert(aid.clone(), node);

        let project = self.project.clone();
        let router = self.handle.clone();
        let child = aid.clone();
        tokio::spawn(async move {
            // A cancelled or panicking setup also rolls back explicitly.
            let mut guard = SpawnGuard {
                router: router.clone(),
                aid: Some(child.clone()),
            };
            let result: Result<()> = async {
                let parent_state = project.store().load_state(&parent).await?;
                let commit = parent_state.context.commit;
                project
                    .duplicate_agent_workdir(&parent, &child, &commit)
                    .await?;
                // the child's frozen copy is what gets committed — a parallel
                // tool call may be writing the parent's workdir right now
                let base = project
                    .mint_spawn_base(&child, &parent_state.context.base, &commit)
                    .await?;
                let assistant = project.assistants().subagent(&parent_state.assistant)?.id;
                let state = AgentState {
                    status: Default::default(),
                    assistant,
                    context: AgentContext {
                        commit,
                        base,
                        history: capture,
                    },
                    pending_messages: Vec::new(),
                };
                // the state rides the same FIFO after the graph record: durable
                // state ⇒ durable graph record — but only if the record itself
                // committed, so check its receiver before writing the state (M6)
                record_write.await?;
                project.store().save_state(&child, &state).await?;
                let agent = Agent::with_mailbox(
                    project.clone(),
                    router.clone(),
                    child.clone(),
                    state,
                    tx,
                    rx,
                );
                let (runtime, task) = agent.prepare();
                router.attach_runtime(child.clone(), runtime).await?;
                task.launch();
                Ok(())
            }
            .await;
            match result {
                Ok(()) => {
                    guard.defuse();
                    drop(done.send(Ok(child.clone())));
                }
                Err(error) => {
                    let error = match router.rollback_spawn(child.clone()).await {
                        Ok(()) => error,
                        Err(rollback) => anyhow::anyhow!(
                            "{error:#}; failed to roll back child {child}: {rollback:#}"
                        ),
                    };
                    guard.defuse();
                    drop(done.send(Err(error)));
                }
            }
        });
    }

    pub fn handle_rollback_spawn(
        &mut self,
        aid: AgentId,
        done: oneshot::Sender<Result<()>>,
    ) {
        if let Some(node) = self.graph.remove(&aid) {
            if let Some(abort) = node.abort {
                abort.abort();
            }
            self.fire_waiters(&aid, Err(RouterError::Unreachable));
        }
        let write = self.project.store().delete_agent(&aid);
        let project = self.project.clone();
        tokio::spawn(async move {
            let result = async {
                write.await?;
                project.delete_agent_workdir(&aid).await
            }
            .await;
            drop(done.send(result));
        });
    }
}

/// Rolls back a cancelled or panicking spawn setup instead of leaving a
/// provisional `Spawning` node.
struct SpawnGuard {
    router: AgentRouterHandle,
    aid: Option<AgentId>,
}

impl SpawnGuard {
    fn defuse(&mut self) {
        self.aid = None;
    }
}

impl Drop for SpawnGuard {
    fn drop(&mut self) {
        if let Some(aid) = self.aid.take() {
            let router = self.router.clone();
            tokio::spawn(async move { drop(router.rollback_spawn(aid).await) });
        }
    }
}

//! `spawn`: a provisional node under the lock, then a detached setup task
//! that captures the parent's workdir/state and launches the child; any
//! failure rolls back through `rollback_spawn`

use anyhow::Result;

use super::peer_message;
use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::router::Router;
use crate::agent::router::TAB_AGENT_CAP;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::NodeStatus;
use crate::llm::history::History;

impl Router {
    /// registers the child synchronously; resolves once the {graph record,
    /// history, workdir} capture is durable
    pub async fn spawn_agent(
        &self,
        parent: &AgentId,
        inherited_history: Option<History>,
        prompt: &str,
    ) -> Result<AgentId> {
        let (aid, record_write, project, app_tx) = {
            let s = &mut *self.lock();
            let root = match s.graph.get(parent) {
                Some(node) if node.status() != NodeStatus::Dead => node.root.clone(),
                Some(_) => anyhow::bail!("agent {parent} is dead"),
                None => anyhow::bail!("unknown agent {parent}"),
            };
            // per-tab count cap; recovery must not depend on
            // remembering ids — the error spells out the list → archive loop
            anyhow::ensure!(
                s.graph.values().filter(|n| n.root == root).count() < TAB_AGENT_CAP,
                "this tab is at its agent cap ({TAB_AGENT_CAP}): archive finished agents to \
                 free slots (list shows every member and its status)"
            );
            let aid = s.allocate();
            let node = AgentNode::new(root, Some(parent.clone()));
            // submitted under the lock, FIFO-ordered ahead of the tail's state
            // save; the tail awaits this write so a failed graph write aborts
            // the spawn instead of leaving state without a graph record
            let record_write = s.project.store().save_graph(&aid, &node.record(false));
            s.graph.insert(aid.clone(), node);
            // a fresh mailbox: the seed parks first, ahead of anything else
            s.deliver(&aid, peer_message(parent, prompt))?;
            (aid, record_write, s.project.clone(), s.app_tx.clone())
        };

        let router = self.clone();
        let parent = parent.clone();
        let child = aid.clone();
        // detached: the setup completes (or rolls back) even if the calling
        // tool is aborted
        let setup = tokio::spawn(async move {
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
                let assistant_id = project
                    .assistants()
                    .subagent(&parent_state.assistant_id)?
                    .id;
                // a fresh child starts like a primary agent, from its own tree
                let history = match inherited_history {
                    Some(history) => history,
                    None => History::new(project.instructions(&child).await?),
                };
                let state = AgentState {
                    status: Default::default(),
                    assistant_id,
                    context: AgentContext {
                        commit,
                        base,
                        history,
                    },
                    pending_messages: Vec::new(),
                };
                // the state rides the same FIFO after the graph record: durable
                // state ⇒ durable graph record — but only if the record itself
                // committed, so check its receiver before writing the state
                record_write.await?;
                project.store().save_state(&child, &state).await?;
                let agent = Agent::new(
                    project.clone(),
                    router.clone(),
                    app_tx,
                    child.clone(),
                    state,
                );
                router.launch(agent)
            }
            .await;
            guard.defuse();
            if let Err(error) = result {
                return Err(match router.rollback_spawn(&child).await {
                    Ok(()) => error,
                    Err(rollback) => {
                        anyhow::anyhow!(
                            "{error:#}; failed to roll back child {child}: {rollback:#}"
                        )
                    }
                });
            }
            Ok(())
        });
        setup.await??;
        Ok(aid)
    }

    /// drop a provisional node: its graph record, state and workdir go too
    pub async fn rollback_spawn(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        let (write, project) = {
            let s = &mut *self.lock();
            s.drop_node(aid);
            (s.project.store().delete_agent(aid), s.project.clone())
        };
        write.await?;
        project.delete_agent_workdir(aid).await
    }
}

/// Rolls back a cancelled or panicking spawn setup instead of leaving a
/// provisional `Spawning` node.
struct SpawnGuard {
    router: Router,
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
            tokio::spawn(async move { drop(router.rollback_spawn(&aid).await) });
        }
    }
}

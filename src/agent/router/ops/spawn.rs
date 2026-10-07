//! `spawn`: a detached setup task that checks the child's workdir out at the
//! requested commit and saves its state, then `start`s it — the node joins
//! the graph only then, so nobody ever sees a spawn in progress; any failure
//! rolls back through `rollback_spawn`

use anyhow::Result;

use super::peer_message;
use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::router::Router;
use crate::agent::router::RouterState;
use crate::agent::router::TAB_AGENT_CAP;
use crate::agent::router::graph::Runtime;
use crate::llm::history::History;

impl Router {
    /// resolves once the child's {state, workdir at `commit`, graph record}
    /// are durable and its runtime started
    pub async fn spawn_agent(
        &self,
        parent: &AgentId,
        commit: &str,
        inherited_history: Option<History>,
        prompt: &str,
    ) -> Result<AgentId> {
        // fail fast; `start` checks again at commit
        let (aid, project, app_tx) = {
            let s = &mut *self.lock();
            s.child_root(parent)?;
            (s.allocate(), s.project.clone(), s.app_tx.clone())
        };
        let seed = peer_message(parent, prompt);

        let router = self.clone();
        let parent = parent.clone();
        let commit = commit.to_string();
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
                // the whole tab shares one snapshot; the child starts at `commit`
                let snapshot = parent_state.context.commit;
                project
                    .spawn_agent_workdir(&child, &snapshot, &commit)
                    .await?;
                let assistant_id = project
                    .assistants()
                    .subagent(&parent_state.assistant_id)?
                    .id;
                // a fresh child reads its instructions from its own tree
                let history = match inherited_history {
                    Some(history) => history,
                    None => History::new_subagent(project.instructions(&child).await?),
                };
                let state = AgentState {
                    assistant_id,
                    context: AgentContext {
                        commit: snapshot,
                        history,
                    },
                    // the seed rides the saved state: the child's startup
                    // `resume` turns on it, and a crash before that can't lose it
                    pending_messages: vec![seed.into()],
                };
                // durable graph record ⇒ durable state; a crash before the
                // record leaves residue for cleanup
                project.store().save_state(&child, &state).await?;
                let agent = Agent::new(
                    project.clone(),
                    router.clone(),
                    app_tx,
                    child.clone(),
                    state,
                );
                router.start(agent, Some(&parent)).await
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

    /// undo a failed start: its node (if any), graph record, state and
    /// workdir
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

/// Rolls back a cancelled or panicking spawn setup instead of leaving its
/// residue.
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

impl RouterState {
    /// the root a new child of `parent` joins, if its tab has room
    pub fn child_root(
        &self,
        parent: &AgentId,
    ) -> Result<AgentId> {
        let root = match self.graph.get(parent) {
            Some(node) if !matches!(node.runtime, Runtime::Dead(_)) => node.root.clone(),
            Some(_) => anyhow::bail!("agent {parent} is dead"),
            None => anyhow::bail!("unknown agent {parent}"),
        };
        // per-tab count cap; recovery must not depend on
        // remembering ids — the error spells out the list → archive loop
        anyhow::ensure!(
            self.graph.values().filter(|n| n.root == root).count() < TAB_AGENT_CAP,
            "this tab is at its agent cap ({TAB_AGENT_CAP}): archive finished agents to \
             free slots (list shows every member and its status)"
        );
        Ok(root)
    }
}

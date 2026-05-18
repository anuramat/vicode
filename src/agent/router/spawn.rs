use anyhow::Context;
use anyhow::Result;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;

use super::AgentRouter;
use super::AgentRouterHandle;
use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::AgentStatus;
use crate::agent::handle::AgentEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::project::Project;

impl AgentRouter {
    /// Dispatch the rest of subagent spawning to a tokio task so the router
    /// loop isn't blocked awaiting the parent's snapshot reply. Registration
    /// of the child runtime goes back through `RouterCommand::Register`, and
    /// the caller's oneshot only fires after registration is queued — so a
    /// follow-up `Submit` from the caller can't race ahead of `Register`.
    pub fn dispatch_spawn_subagent(
        &self,
        parent_aid: AgentId,
        inherit_context: bool,
        reply: oneshot::Sender<Result<(AgentId, HistoryGeneration)>>,
    ) {
        let Some(runtime) = self.runtimes.get(&parent_aid) else {
            drop(reply.send(Err(anyhow::anyhow!("unknown agent {parent_aid}"))));
            return;
        };
        let parent_tx = runtime.tx.clone();
        let router = self.handle.clone();
        let project = self.project.clone();
        tokio::spawn(async move {
            let result =
                spawn_subagent_async(parent_aid, parent_tx, router, project, inherit_context).await;
            drop(reply.send(result));
        });
    }
}

async fn spawn_subagent_async(
    parent_aid: AgentId,
    parent_tx: Sender<AgentEvent>,
    router: AgentRouterHandle,
    project: Project,
    inherit_context: bool,
) -> Result<(AgentId, HistoryGeneration)> {
    let snap = {
        let (snap_tx, snap_rx) = oneshot::channel();
        parent_tx.send(AgentEvent::SnapshotRequest(snap_tx)).await?;
        let snap = snap_rx.await?;
        // NOTE this shouldn't be possible -- subagent tool should be hidden if max_depth is 0
        anyhow::ensure!(
            snap.max_depth > 0,
            "max subagent depth reached in {parent_aid}"
        );
        snap
    };

    let child_aid = AgentId::new(&project).await?;

    let agent = {
        let history = snap.history.subagent(inherit_context);
        let state = AgentState {
            status: AgentStatus::default(),
            assistant: ASSISTANT_POOL
                .get()
                .context("assistant pool not initialized")?
                .next_subagent(&snap.assistant_id)?,
            max_depth: snap.max_depth - 1,
            context: AgentContext {
                commit: snap.commit.clone(),
                history,
            },
        };
        project
            .duplicate_agent_workdir(&parent_aid, &child_aid, &snap.commit, false)
            .await?;
        let agent = Agent::from_state(project, router.clone(), child_aid.clone(), state);
        agent.save().await?;
        agent
    };
    let generation = agent.history().generation();
    let runtime = agent.spawn();
    router.register(child_aid.clone(), runtime).await?;
    Ok((child_aid, generation))
}

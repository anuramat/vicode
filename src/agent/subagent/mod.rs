pub mod replica;
/// SLOP `result` module is vibecoded
#[allow(deprecated, clippy::pedantic, clippy::nursery, clippy::style)]
pub mod result;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::oneshot;

use crate::agent::handle::TurnResult;
use crate::agent::handle::UserPrompt;
use crate::agent::id::AgentId;
use crate::agent::router::AgentRouterHandle;
use crate::agent::subagent::result::diff;
use crate::project::Project;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentResult {
    pub output: String,
    pub diff: String,
}

#[derive(Debug)]
pub struct SubagentHandle {
    pub id: AgentId,
    parent_aid: AgentId,
    project: Project,
    router: AgentRouterHandle,
    turn: oneshot::Receiver<TurnResult>,
}

impl SubagentHandle {
    /// Await the subagent's turn and unconditionally delete its router entry.
    /// The runtime is removed even on error so callers can't leak it by
    /// failing to handle a `Failed`/`Aborted` outcome.
    pub async fn wait(self) -> Result<SubagentResult> {
        let aid = self.id.clone();
        let result = self.turn.await.context("subagent channel closed");
        drop(self.router.delete(aid.clone()).await);
        let output = match result? {
            TurnResult::Success { last_text } => last_text.unwrap_or_default(),
            TurnResult::Failed(msg) => anyhow::bail!("subagent error: {msg}"),
            TurnResult::Aborted => anyhow::bail!("subagent aborted"),
        };
        let diff = diff(&self.project, &self.parent_aid, &aid)?;
        Ok(SubagentResult { output, diff })
    }
}

/// Spawn a subagent under `parent_aid` via the router and submit `prompt`.
/// Returns a handle whose oneshot fires when the subagent's turn completes.
/// The caller MUST drive [`SubagentHandle::wait`] (or otherwise call
/// `router.delete`) to avoid leaking the spawned runtime.
pub async fn spawn_and_submit(
    router: &AgentRouterHandle,
    project: &Project,
    parent_aid: &AgentId,
    prompt: String,
    inherit_context: bool,
) -> Result<SubagentHandle> {
    let (child, generation) = router
        .spawn_subagent(parent_aid.clone(), inherit_context)
        .await?;
    let turn = router
        .submit_oneshot(
            child.clone(),
            UserPrompt {
                text: prompt,
                multiplier: 1,
                generation,
            },
        )
        .await?;
    Ok(SubagentHandle {
        id: child,
        parent_aid: parent_aid.clone(),
        project: project.clone(),
        router: router.clone(),
        turn,
    })
}

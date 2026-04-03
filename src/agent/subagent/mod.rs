// SLOP
pub mod candidate;

use anyhow::Result;
use tokio::sync::mpsc::channel;

use super::handle::ExternalEvent;
use crate::agent::Agent;
use crate::agent::AgentEvent;
use crate::agent::AgentKind;
use crate::agent::AgentState;
use crate::agent::AgentStatus;
use crate::agent::AgentTopology;
use crate::agent::handle::ParentEvent;
use crate::agent::handle::UserPrompt;
use crate::agent::id::AgentId;
use crate::agent::init::channel_parent_sink;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::project::Project;

pub async fn run_child(
    project: Project,
    parent: &AgentId,
    aid: &AgentId,
    state: &AgentState,
    text: Option<String>,
) -> Result<String> {
    let state = AgentState {
        status: AgentStatus::Idle,
        assistant: ASSISTANT_POOL.get().unwrap().assistant(
            &ASSISTANT_POOL
                .get()
                .unwrap()
                .next_subagent(&state.assistant.id),
        )?,
        topology: AgentTopology {
            kind: AgentKind::Subagent {
                parent: parent.clone(),
            },
            children: Vec::new(),
        },
        context: state.context.clone(),
    };
    project.duplicate_agent(parent, aid, &state, false).await?;

    let (parent_tx, mut parent_rx) = channel(100);
    let agent = Agent::load(project.clone(), channel_parent_sink(parent_tx), aid.clone()).await?;
    let child_tx = agent.tx.clone();
    agent.spawn();

    child_tx
        .send(AgentEvent::External(ExternalEvent::Submit(UserPrompt {
            text,
            multiplier: 1,
            generation: state.context.history.generation(),
        })))
        .await?;

    loop {
        match parent_rx.recv().await {
            Some(ParentEvent::StatusUpdate(s)) if s.idle() => break,
            Some(_) => continue,
            None => anyhow::bail!("subagent channel closed before turn completion"),
        }
    }

    candidate::response(&project, parent, aid).await
}

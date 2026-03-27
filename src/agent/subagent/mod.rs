/// SLOP
pub mod candidate;

use anyhow::Result;
use tokio::sync::mpsc::channel;
use tokio::task::JoinSet;

use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentEvent;
use crate::agent::AgentKind;
use crate::agent::AgentState;
use crate::agent::AgentTopology;
use crate::agent::handle::ParentEvent;
use crate::agent::handle::UserPrompt;
use crate::agent::id::AgentId;
use crate::agent::init::duplicate;
use crate::llm::provider::assistant::ASSISTANT_POOL;

pub async fn run_child(
    parent: &AgentId,
    aid: &AgentId,
    context: &AgentContext,
    text: Option<String>,
) -> Result<String> {
    let state = AgentState {
        topology: AgentTopology {
            kind: AgentKind::Subagent {
                parent: parent.clone(),
            },
            children: Vec::new(),
        },
        context: AgentContext {
            assistant_id: ASSISTANT_POOL
                .get()
                .unwrap()
                .next_subagent(&context.assistant_id),
            ..context.clone()
        },
    };
    duplicate(parent, aid, &state, false).await?;

    let (parent_tx, mut parent_rx) = channel(100);
    let agent = Agent::load(parent_tx, aid.clone()).await?;
    let child_tx = agent.tx.clone();
    let mut tasks = JoinSet::new();
    tasks.spawn(agent.run());

    child_tx
        .send(AgentEvent::Submit(UserPrompt {
            text,
            multiplier: 1,
            generation: context.history.generation(),
        }))
        .await?;

    loop {
        match parent_rx.recv().await {
            Some((completed, ParentEvent::TurnComplete)) if completed == *aid => break,
            Some(_) => continue,
            None => anyhow::bail!("subagent channel closed before turn completion"),
        }
    }
    tasks.abort_all();

    candidate::response(parent, aid).await
}

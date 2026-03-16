use anyhow::Result;
use tokio::task::JoinSet;

use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::subagent;

const REPORT_HEADER_PROMPT: &str = "Here are multiple implementations for the requested changes.
Please review them and provide a single, consolidated implementation that combines the best aspects
of each. Ensure that the final implementation is efficient, well-structured, and adheres to best
coding practices. If there are any conflicting approaches, choose the one that is most effective
and explain your reasoning briefly.\n\n";

#[derive(Debug)]
pub struct ReplicaResult {
    pub report: String,
}

pub async fn run_replicas(
    parent: AgentId,
    context: AgentContext,
    children: Vec<AgentId>,
) -> Result<ReplicaResult> {
    let mut tasks = JoinSet::new();
    for aid in children {
        let parent = parent.clone();
        let context = context.clone();
        tasks.spawn(async move { subagent::run_child(&parent, &aid, &context, None).await });
    }
    let mut results = Vec::new();
    while let Some(res) = tasks.join_next().await {
        results.push(res??);
    }
    Ok(ReplicaResult {
        report: format!("{}{}", REPORT_HEADER_PROMPT, results.join("\n\n")),
    })
}

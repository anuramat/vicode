use anyhow::Result;

use super::SubagentHandle;

const REPORT_HEADER_PROMPT: &str = "Here are multiple implementations for the requested changes.
Please review them and provide a single, consolidated implementation that combines the best aspects
of each. Ensure that the final implementation is efficient, well-structured, and adheres to best
coding practices. If there are any conflicting approaches, choose the one that is most effective
and explain your reasoning briefly.\n\n";

#[derive(Debug)]
pub struct ReplicaResult {
    pub report: String,
}

pub async fn run_replicas(handles: Vec<SubagentHandle>) -> Result<ReplicaResult> {
    let mut results = Vec::new();
    for handle in handles {
        let aid = handle.id.clone();
        let result = handle.wait().await?;
        // TODO change format
        results.push(format!(
            "<implementation id={aid}>\n{}\n```diff\n{}```\n</implementation>",
            result.output, result.diff
        ));
    }
    Ok(ReplicaResult {
        report: format!("{}{}", REPORT_HEADER_PROMPT, results.join("\n\n")),
    })
}

use anyhow::Result;
use futures::future::join_all;

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

/// Wait for every replica even if some fail; each `SubagentHandle::wait`
/// removes its runtime from the router on completion, so a failing replica no
/// longer drops sibling handles unwaited.

pub async fn run_replicas(handles: Vec<SubagentHandle>) -> Result<ReplicaResult> {
    let entries: Vec<String> = join_all(handles.into_iter().map(|h| async move {
        let aid = h.id.clone();
        match h.wait().await {
            // TODO change format
            Ok(r) => format!(
                "<implementation id={aid}>\n{}\n```diff\n{}```\n</implementation>",
                r.output, r.diff
            ),
            Err(e) => format!("<implementation id={aid}>\nerror: {e}\n</implementation>"),
        }
    }))
    .await;
    Ok(ReplicaResult {
        report: format!("{}{}", REPORT_HEADER_PROMPT, entries.join("\n\n")),
    })
}

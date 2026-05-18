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

/// Wait for every replica even if some fail; each `SubagentHandle::wait`
/// removes its runtime from the router on completion, so a failing replica no
/// longer drops sibling handles unwaited.
#[cfg(test)]
mod tests {
    impl SubagentHandle {
        /// Build a handle with a synthetic turn channel — used by tests that need
        /// to drive `run_replicas` outcomes without running a real subagent loop.
        pub fn new_for_test(
            id: crate::agent::id::AgentId,
            parent_aid: crate::agent::id::AgentId,
            project: crate::project::Project,
            router: crate::agent::router::AgentRouterHandle,
            turn: tokio::sync::oneshot::Receiver<crate::agent::handle::TurnResult>,
        ) -> Self {
            Self {
                id,
                parent_aid,
                project,
                router,
                turn,
            }
        }
    }

    use futures::future::AbortHandle;
    use tokio::sync::mpsc::channel;
    use tokio::sync::oneshot;

    use super::*;
    use crate::agent::handle::TurnResult;
    use crate::agent::id::AgentId;
    use crate::agent::router::AgentRouter;
    use crate::agent::router::RuntimeHandle;
    use crate::project::Project;

    #[tokio::test]
    async fn failing_replica_does_not_leak_sibling_runtimes() {
        let project = Project::new_test().unwrap();
        let (app_tx, _app_rx) = channel(8);
        let router = AgentRouter::spawn(app_tx, project.clone());
        let parent_aid = AgentId::from(format!("replica-parent-{}", uuid::Uuid::new_v4()));

        // register N fake runtimes
        let n = 3;
        let mut aids = Vec::with_capacity(n);
        let mut handles = Vec::with_capacity(n);
        let mut senders = Vec::with_capacity(n);
        for i in 0..n {
            let aid = AgentId::from(format!("replica-{}-{}", i, uuid::Uuid::new_v4()));
            let (runtime_tx, _rx) = channel(8);
            let (abort, _reg) = AbortHandle::new_pair();
            router
                .register(aid.clone(), RuntimeHandle::new(runtime_tx, abort))
                .await
                .unwrap();
            let (turn_tx, turn_rx) = oneshot::channel();
            handles.push(SubagentHandle::new_for_test(
                aid.clone(),
                parent_aid.clone(),
                project.clone(),
                router.clone(),
                turn_rx,
            ));
            senders.push(turn_tx);
            aids.push(aid);
        }

        // first replica fails, the rest also fail (Failed avoids the diff() path
        // which would need real workdirs); the leak fix is what's under test
        for sender in senders {
            sender.send(TurnResult::Failed("boom".into())).unwrap();
        }

        run_replicas(handles).await.unwrap();

        // every aid should now be absent from the router
        for aid in aids {
            assert!(router.delete(aid).await.is_err());
        }
    }
}

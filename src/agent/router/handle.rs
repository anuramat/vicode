use anyhow::Result;
use tokio::sync::oneshot;

use super::AgentRouter;
use super::RouterCommand;
use super::RuntimeHandle;
use crate::agent::AgentId;
use crate::agent::handle::AgentEvent;
use crate::agent::handle::ExternalEvent;

impl AgentRouter {
    pub async fn handle(
        &mut self,
        cmd: RouterCommand,
    ) {
        match cmd {
            RouterCommand::Register { aid, runtime } => self.handle_register(aid, runtime),
            RouterCommand::Forward { aid, event } => self.handle_forward(aid, event).await,
            RouterCommand::SpawnSubagent {
                parent,
                inherit_context,
                reply,
            } => self.dispatch_spawn_subagent(parent, inherit_context, reply),
            RouterCommand::Allocate { done } => self.handle_allocate(done),
            RouterCommand::Delete { aid, done } => self.handle_delete(&aid, done),
        }
    }

    fn handle_register(
        &mut self,
        aid: AgentId,
        runtime: RuntimeHandle,
    ) {
        self.agent_ids.insert(aid.clone());
        if let Some(prev) = self.runtimes.insert(aid, runtime) {
            prev.abort.abort();
        }
    }

    fn handle_allocate(
        &mut self,
        done: oneshot::Sender<Result<AgentId>>,
    ) {
        for aid in AgentId::generate() {
            if self.agent_ids.insert(aid.clone()) {
                drop(done.send(Ok(aid)));
                return;
            }
        }
        drop(done.send(Err(anyhow::anyhow!(
            "{} collisions when generating agent id",
            crate::agent::id::PATIENCE
        ))));
    }

    async fn handle_forward(
        &mut self,
        aid: AgentId,
        event: ExternalEvent,
    ) {
        let Some(runtime) = self.runtimes.get(&aid) else {
            tracing::error!("forward: unknown agent {aid}");
            return;
        };
        if let Err(e) = runtime.tx.send(AgentEvent::External(event)).await {
            self.runtimes.remove(&aid);
            tracing::error!("forward to {aid} failed: {e}");
        }
    }

    fn handle_delete(
        &mut self,
        aid: &AgentId,
        done: oneshot::Sender<Result<()>>,
    ) {
        let result = if let Some(runtime) = self.runtimes.remove(aid) {
            runtime.abort.abort();
            Ok(())
        } else {
            Err(anyhow::anyhow!("unknown agent {aid}"))
        };
        drop(done.send(result));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use futures::future::AbortHandle;
    use tokio::sync::mpsc::Receiver;
    use tokio::sync::mpsc::channel;

    use super::super::AgentRouterHandle;
    use super::super::RuntimeHandle;
    use super::super::SubagentSpawnSnapshot;
    use super::*;
    use crate::agent::handle::UserPrompt;
    use crate::llm::history::History;
    use crate::project::Project;
    use crate::project::layout::LayoutTrait;

    fn fake_runtime() -> (RuntimeHandle, Receiver<AgentEvent>) {
        let (tx, rx) = channel(8);
        let (abort, _reg) = AbortHandle::new_pair();
        (RuntimeHandle::new(tx, abort), rx)
    }

    fn empty_router() -> AgentRouter {
        let (tx, rx) = channel(8);
        let (app_tx, app_rx) = channel(8);
        std::mem::forget(app_rx);
        let handle = AgentRouterHandle { tx, app_tx };
        let project = Project::new_test().unwrap().0;
        AgentRouter {
            agent_ids: Default::default(),
            runtimes: HashMap::new(),
            rx,
            handle,
            project,
        }
    }

    #[tokio::test]
    async fn submit_to_dead_runtime_clears_entry_and_closes_oneshot() {
        let mut router = empty_router();
        let aid = AgentId::from("dead-submit".to_string());
        let (runtime, rx) = fake_runtime();
        drop(rx);
        router.runtimes.insert(aid.clone(), runtime);

        let (done, done_rx) = oneshot::channel();
        router
            .handle(RouterCommand::Forward {
                aid: aid.clone(),
                event: ExternalEvent::Submit(
                    UserPrompt {
                        text: "".into(),
                        multiplier: 1,
                        generation: 0,
                    },
                    Some(done),
                ),
            })
            .await;

        assert!(done_rx.await.is_err());
        assert!(!router.runtimes.contains_key(&aid));
    }

    #[tokio::test]
    async fn forward_to_dead_runtime_clears_entry() {
        let mut router = empty_router();
        let aid = AgentId::from("dead-forward".to_string());
        let (runtime, rx) = fake_runtime();
        drop(rx);
        router.runtimes.insert(aid.clone(), runtime);

        router
            .handle(RouterCommand::Forward {
                aid: aid.clone(),
                event: ExternalEvent::Abort,
            })
            .await;

        assert!(!router.runtimes.contains_key(&aid));
    }

    #[tokio::test]
    async fn delete_aborts_runtime_and_removes_entry() {
        let mut router = empty_router();
        let aid = AgentId::from("live-delete".to_string());
        let (tx, _rx) = channel(8);
        let (abort, reg) = AbortHandle::new_pair();
        let pending = futures::future::Abortable::new(futures::future::pending::<()>(), reg);
        let task = tokio::spawn(pending);
        router
            .runtimes
            .insert(aid.clone(), RuntimeHandle::new(tx, abort));

        let (done, done_rx) = oneshot::channel();
        router
            .handle(RouterCommand::Delete {
                aid: aid.clone(),
                done,
            })
            .await;

        assert!(done_rx.await.unwrap().is_ok());
        assert!(!router.runtimes.contains_key(&aid));
        assert!(task.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn delete_unknown_agent_errors() {
        let mut router = empty_router();
        let (done, done_rx) = oneshot::channel();
        router
            .handle(RouterCommand::Delete {
                aid: AgentId::from("missing".to_string()),
                done,
            })
            .await;
        assert!(done_rx.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn spawn_subagent_unknown_parent_errors() {
        let mut router = empty_router();
        let (reply, reply_rx) = oneshot::channel();

        router
            .handle(RouterCommand::SpawnSubagent {
                parent: AgentId::from("missing".to_string()),
                inherit_context: false,
                reply,
            })
            .await;

        assert!(reply_rx.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn spawn_subagent_snapshots_parent_and_registers_child() {
        let project = Project::new_test().unwrap().0;
        let parent_aid = AgentId::from(format!("parent-{}", uuid::Uuid::new_v4()));
        let parent_workdir = project.agent_workdir(&parent_aid);
        tokio::fs::create_dir_all(&parent_workdir).await.unwrap();
        let repo = git2::Repository::open(project.root()).unwrap();
        let commit_str = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();

        let (app_tx, _app_rx) = channel(8);
        let handle = AgentRouter::spawn(app_tx, project.clone(), Default::default());

        let (parent_runtime, mut parent_rx) = fake_runtime();
        handle
            .register(parent_aid.clone(), parent_runtime)
            .await
            .unwrap();

        // Drive a fake parent: respond to SnapshotRequest with a stub.
        let snap_commit = commit_str.clone();
        let parent_task = tokio::spawn(async move {
            let evt = parent_rx.recv().await.unwrap();
            let AgentEvent::SnapshotRequest(reply) = evt else {
                panic!("expected SnapshotRequest, got {evt:?}");
            };
            drop(reply.send(SubagentSpawnSnapshot {
                commit: snap_commit,
                assistant_id: "test".into(),
                history: History::new("instr".into()),
                max_depth: 1,
            }));
        });

        let (child_aid, _generation) = handle
            .spawn_subagent(parent_aid.clone(), false)
            .await
            .unwrap();
        parent_task.await.unwrap();

        assert_ne!(child_aid, parent_aid);
        // registered child is observable via successful delete
        handle.delete(child_aid).await.unwrap();
    }

    #[tokio::test]
    async fn register_overwrite_aborts_previous_runtime() {
        let mut router = empty_router();
        let aid = AgentId::from("dup".to_string());

        let (tx1, _rx1) = channel(8);
        let (abort1, reg1) = AbortHandle::new_pair();
        let pending = futures::future::Abortable::new(futures::future::pending::<()>(), reg1);
        let task1 = tokio::spawn(pending);
        router
            .handle(RouterCommand::Register {
                aid: aid.clone(),
                runtime: RuntimeHandle::new(tx1, abort1),
            })
            .await;

        let (tx2, mut rx2) = channel(8);
        let (abort2, _reg2) = AbortHandle::new_pair();
        router
            .handle(RouterCommand::Register {
                aid: aid.clone(),
                runtime: RuntimeHandle::new(tx2, abort2),
            })
            .await;

        assert!(task1.await.unwrap().is_err());

        // subsequent forwards land on the new runtime
        router
            .handle(RouterCommand::Forward {
                aid: aid.clone(),
                event: ExternalEvent::Abort,
            })
            .await;
        let evt = rx2.recv().await.unwrap();
        assert!(matches!(evt, AgentEvent::External(ExternalEvent::Abort)));
    }
}

use anyhow::Context;
use anyhow::Result;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;

use super::AgentRouter;
use super::AgentRouterHandle;
use super::RouterCommand;
use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::AgentStatus;
use crate::agent::handle::AgentEvent;
use crate::agent::handle::ExternalEvent;
use crate::agent::handle::TurnResult;
use crate::llm::history::HistoryGeneration;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::project::Project;

impl AgentRouter {
    pub async fn handle(
        &mut self,
        cmd: RouterCommand,
    ) {
        match cmd {
            RouterCommand::Register { aid, runtime } => {
                if let Some(prev) = self.runtimes.insert(aid, runtime) {
                    prev.abort.abort();
                }
            }
            RouterCommand::Forward { aid, event } => {
                let Some(runtime) = self.runtimes.get(&aid) else {
                    tracing::error!("forward: unknown agent {aid}");
                    return;
                };
                // clone tx so we don't hold &self.runtimes across the await
                let tx = runtime.tx.clone();
                if let Err(e) = tx.send(AgentEvent::External(event)).await {
                    self.runtimes.remove(&aid);
                    tracing::error!("forward to {aid} failed: {e}");
                }
            }
            RouterCommand::Submit { aid, prompt, done } => {
                let Some(runtime) = self.runtimes.get(&aid) else {
                    drop(done.send(TurnResult::Failed(format!("unknown agent {aid}"))));
                    return;
                };
                // clone tx so we don't hold &self.runtimes across the await
                let tx = runtime.tx.clone();
                let send = tx
                    .send(AgentEvent::External(ExternalEvent::Submit(
                        prompt,
                        Some(done),
                    )))
                    .await;
                if let Err(e) = send {
                    self.runtimes.remove(&aid);
                    let AgentEvent::External(ExternalEvent::Submit(_, Some(done))) = e.0 else {
                        unreachable!()
                    };
                    drop(done.send(TurnResult::Failed("runtime mailbox closed".into())));
                }
            }
            RouterCommand::SpawnSubagent {
                parent,
                inherit_context,
                reply,
            } => {
                self.dispatch_spawn_subagent(parent, inherit_context, reply);
            }
            RouterCommand::Delete { aid, done } => {
                let result = if let Some(runtime) = self.runtimes.remove(&aid) {
                    runtime.abort.abort();
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("unknown agent {aid}"))
                };
                drop(done.send(result));
            }
        }
    }

    /// Dispatch the rest of subagent spawning to a tokio task so the router
    /// loop isn't blocked awaiting the parent's snapshot reply. Registration
    /// of the child runtime goes back through `RouterCommand::Register`, and
    /// the caller's oneshot only fires after registration is queued — so a
    /// follow-up `Submit` from the caller can't race ahead of `Register`.
    fn dispatch_spawn_subagent(
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
    let (snap_tx, snap_rx) = oneshot::channel();
    parent_tx.send(AgentEvent::SnapshotRequest(snap_tx)).await?;
    let snap = snap_rx.await?;
    anyhow::ensure!(
        snap.max_depth > 0,
        "parent {parent_aid} has exhausted its subagent depth budget"
    );
    let assistant = ASSISTANT_POOL
        .get()
        .context("assistant pool not initialized")?
        .next_subagent(&snap.assistant_id)?;
    let history = snap.history.subagent(inherit_context);
    let generation = history.generation();
    let state = AgentState {
        status: AgentStatus::default(),
        assistant,
        max_depth: snap.max_depth - 1,
        context: AgentContext {
            commit: snap.commit.clone(),
            history,
        },
    };
    let child_aid = AgentId::new(&project).await?;
    project
        .duplicate_agent_workdir(&parent_aid, &child_aid, &snap.commit, false)
        .await?;
    let agent = Agent::from_state(project, router.clone(), child_aid.clone(), state);
    agent.save().await?;
    let runtime = agent.spawn();
    router.register(child_aid.clone(), runtime).await?;
    Ok((child_aid, generation))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use futures::future::AbortHandle;
    use tokio::sync::mpsc::Receiver;
    use tokio::sync::mpsc::channel;

    use super::super::RuntimeHandle;
    use super::super::SubagentSpawnSnapshot;
    use super::*;
    use crate::agent::handle::UserPrompt;
    use crate::llm::history::History;
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
        AgentRouter {
            runtimes: HashMap::new(),
            rx,
            handle,
            project: Project::new_test().unwrap(),
        }
    }

    #[tokio::test]
    async fn submit_to_dead_runtime_clears_entry_and_fires_failed() {
        let mut router = empty_router();
        let aid = AgentId::from("dead-submit".to_string());
        let (runtime, rx) = fake_runtime();
        drop(rx);
        router.runtimes.insert(aid.clone(), runtime);

        let (done, done_rx) = oneshot::channel();
        router
            .handle(RouterCommand::Submit {
                aid: aid.clone(),
                prompt: UserPrompt {
                    text: "".into(),
                    multiplier: 1,
                    generation: 0,
                },
                done,
            })
            .await;

        assert!(matches!(done_rx.await.unwrap(), TurnResult::Failed(_)));
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
        use crate::config::Config;
        use crate::llm::provider::assistant::AssistantPool;

        ASSISTANT_POOL
            .get_or_init(|| async {
                AssistantPool::from_config(
                    &Config::parse_with_defaults(
                        r#"
                primary_assistant = ["test"]
                shell_cmd = ["bash", "-c"]

                [sandbox]
                kind = "bwrap"
                bin = "bwrap"
                args = []
                stages = []

                [providers.main]
                api = "responses"
                base_url = "https://api.example.com/v1"

                [assistants.test]
                provider = "main"
                model = "gpt-test"
                window = 1
                "#,
                    )
                    .unwrap(),
                )
                .await
                .unwrap()
            })
            .await;

        let project = Project::new_test().unwrap();
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
        let handle = AgentRouter::spawn(app_tx, project.clone());

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

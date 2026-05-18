use std::collections::HashMap;

use anyhow::Context;
use anyhow::Result;
use futures::future::AbortHandle;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio::sync::oneshot;

use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::AgentStatus;
use crate::agent::AgentVisibility;
use crate::agent::handle::AgentEvent;
use crate::agent::handle::ExternalEvent;
use crate::agent::handle::TurnResult;
use crate::agent::handle::UserPrompt;
use crate::llm::history::History;
use crate::llm::history::HistoryGeneration;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::project::Project;
use crate::tui::app::AppEvent;

const CHANNEL_CAPACITY: usize = 100;

#[derive(Debug)]
pub struct RuntimeHandle {
    tx: Sender<AgentEvent>,
    abort: AbortHandle,
}

impl RuntimeHandle {
    pub fn new(
        tx: Sender<AgentEvent>,
        abort: AbortHandle,
    ) -> Self {
        Self { tx, abort }
    }
}

/// Snapshot of parent state needed to build a hidden subagent.
#[derive(Debug)]
pub struct SubagentSpawnSnapshot {
    pub commit: String,
    pub assistant_id: String,
    pub history: History,
}

#[derive(Debug)]
pub enum RouterCommand {
    Register {
        aid: AgentId,
        runtime: RuntimeHandle,
    },
    Forward {
        aid: AgentId,
        event: ExternalEvent,
    },
    Submit {
        aid: AgentId,
        prompt: UserPrompt,
        done: oneshot::Sender<TurnResult>,
    },
    SpawnSubagent {
        parent: AgentId,
        inherit_context: bool,
        reply: oneshot::Sender<Result<(AgentId, HistoryGeneration)>>,
    },
    Delete {
        aid: AgentId,
        done: oneshot::Sender<Result<()>>,
    },
}

pub struct AgentRouter {
    runtimes: HashMap<AgentId, RuntimeHandle>,
    rx: Receiver<RouterCommand>,
    handle: AgentRouterHandle,
    project: Project,
}

#[derive(Clone, Debug)]
pub struct AgentRouterHandle {
    tx: Sender<RouterCommand>,
    app_tx: Sender<AppEvent>,
}

impl AgentRouterHandle {
    pub fn app_tx(&self) -> &Sender<AppEvent> {
        &self.app_tx
    }

    pub async fn register(
        &self,
        aid: AgentId,
        runtime: RuntimeHandle,
    ) -> Result<()> {
        self.tx
            .send(RouterCommand::Register { aid, runtime })
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    pub async fn forward(
        &self,
        aid: AgentId,
        event: ExternalEvent,
    ) -> Result<()> {
        self.tx
            .send(RouterCommand::Forward { aid, event })
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// Snapshot `parent` and create a hidden subagent under it. The router
    /// owns the entire spawn flow; the parent serves a snapshot of its state
    /// but does not register the child.
    /// Returns the new id and the freshly-seeded history generation.
    pub async fn spawn_subagent(
        &self,
        parent: AgentId,
        inherit_context: bool,
    ) -> Result<(AgentId, HistoryGeneration)> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::SpawnSubagent {
                parent,
                inherit_context,
                reply,
            })
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        rx.await.map_err(|e| anyhow::anyhow!(e.to_string()))?
    }

    /// Submit a prompt and get a oneshot receiver for the turn result.
    pub async fn submit_oneshot(
        &self,
        aid: AgentId,
        prompt: UserPrompt,
    ) -> Result<oneshot::Receiver<TurnResult>> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::Submit { aid, prompt, done })
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(rx)
    }

    pub async fn delete(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::Delete { aid, done })
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        rx.await.map_err(|e| anyhow::anyhow!(e.to_string()))?
    }
}

impl AgentRouter {
    pub fn spawn(
        app_tx: Sender<AppEvent>,
        project: Project,
    ) -> AgentRouterHandle {
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        let handle = AgentRouterHandle { tx, app_tx };
        let router = Self {
            runtimes: HashMap::new(),
            rx,
            handle: handle.clone(),
            project,
        };
        tokio::spawn(router.run());
        handle
    }

    /// Construct a handle backed by dead-letter channels — for tests that
    /// instantiate Agents without running a real router/app.

    /// Like `test_handle` but caller controls the app channel so test code can
    /// observe `ParentEvent`s emitted by the agent.

    async fn run(mut self) {
        while let Some(cmd) = self.rx.recv().await {
            self.handle(cmd).await;
        }
    }

    async fn handle(
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
                    .send(AgentEvent::External(ExternalEvent::SubmitWithCompletion(
                        prompt, done,
                    )))
                    .await;
                if let Err(e) = send {
                    self.runtimes.remove(&aid);
                    let AgentEvent::External(ExternalEvent::SubmitWithCompletion(_, done)) = e.0
                    else {
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
    parent_tx
        .send(AgentEvent::SnapshotRequest(snap_tx))
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let snap = snap_rx.await.map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let assistant = ASSISTANT_POOL
        .get()
        .context("assistant pool not initialized")?
        .next_subagent(&snap.assistant_id)?;
    let history = snap.history.subagent(inherit_context);
    let generation = history.generation();
    let state = AgentState {
        status: AgentStatus::default(),
        assistant,
        visibility: AgentVisibility::Hidden,
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


use std::collections::HashMap;
use std::collections::HashSet;

use anyhow::Result;
use futures::future::AbortHandle;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio::sync::oneshot;

use crate::agent::AgentId;
use crate::agent::handle::AgentEvent;
use crate::agent::handle::ExternalEvent;
use crate::agent::handle::TurnResult;
use crate::agent::handle::UserPrompt;
use crate::llm::history::History;
use crate::llm::history::HistoryGeneration;
use crate::project::Project;
use crate::tui::app::AppEvent;

mod handle;
mod spawn;

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
    pub max_depth: u32,
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
    SpawnSubagent {
        parent: AgentId,
        inherit_context: bool,
        reply: oneshot::Sender<Result<(AgentId, HistoryGeneration)>>,
    },
    Allocate {
        done: oneshot::Sender<Result<AgentId>>,
    },
    Delete {
        aid: AgentId,
        done: oneshot::Sender<Result<()>>,
    },
}

pub struct AgentRouter {
    agent_ids: HashSet<AgentId>,
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
            .await?;
        Ok(())
    }

    pub async fn forward(
        &self,
        aid: AgentId,
        event: ExternalEvent,
    ) -> Result<()> {
        self.tx.send(RouterCommand::Forward { aid, event }).await?;
        Ok(())
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
            .await?;
        rx.await?
    }

    pub async fn allocate_agent_id(&self) -> Result<AgentId> {
        let (done, rx) = oneshot::channel();
        self.tx.send(RouterCommand::Allocate { done }).await?;
        rx.await?
    }

    /// Submit a prompt and get a oneshot receiver for the turn result.
    pub async fn submit_oneshot(
        &self,
        aid: AgentId,
        prompt: UserPrompt,
    ) -> Result<oneshot::Receiver<TurnResult>> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::Forward {
                aid,
                event: ExternalEvent::Submit(prompt, Some(done)),
            })
            .await?;
        Ok(rx)
    }

    pub async fn delete(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        let (done, rx) = oneshot::channel();
        self.tx.send(RouterCommand::Delete { aid, done }).await?;
        rx.await?
    }
}

impl AgentRouter {
    pub fn spawn(
        app_tx: Sender<AppEvent>,
        project: Project,
        agent_ids: HashSet<AgentId>,
    ) -> AgentRouterHandle {
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        let handle = AgentRouterHandle { tx, app_tx };
        let router = Self {
            agent_ids,
            runtimes: HashMap::new(),
            rx,
            handle: handle.clone(),
            project,
        };
        tokio::spawn(router.run());
        handle
    }

    async fn run(mut self) {
        while let Some(cmd) = self.rx.recv().await {
            self.handle(cmd).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl AgentRouter {
        /// Construct a handle backed by dead-letter channels — for tests that
        /// instantiate Agents without running a real router/app.
        pub fn test_handle() -> AgentRouterHandle {
            let (app_tx, app_rx) = channel(CHANNEL_CAPACITY);
            std::mem::forget(app_rx);
            Self::test_handle_with_app_tx(app_tx)
        }

        /// Like `test_handle` but caller controls the app channel so test code can
        /// observe `ParentEvent`s emitted by the agent.
        pub fn test_handle_with_app_tx(app_tx: Sender<AppEvent>) -> AgentRouterHandle {
            let (tx, rx) = channel(CHANNEL_CAPACITY);
            std::mem::forget(rx);
            AgentRouterHandle { tx, app_tx }
        }
    }
}

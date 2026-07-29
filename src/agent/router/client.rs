//! the handle's request methods: each mints one command and awaits its reply

use anyhow::Result;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;

use super::AgentRouterHandle;
use super::RouterCommand;
use super::RuntimeHandle;
use crate::agent::AgentId;
use crate::agent::handle::ExternalEvent;
use crate::agent::router::api::ListEntry;
use crate::agent::router::api::RouterError;
use crate::agent::router::api::WaitResult;
use crate::agent::router::graph::NodeStatus;
use crate::agent::router::graph::StatusPing;
use crate::llm::history::History;
use crate::tui::app::AppEvent;

impl AgentRouterHandle {
    pub fn app_tx(&self) -> &Sender<AppEvent> {
        &self.app_tx
    }

    pub async fn register_root(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::RegisterRoot { aid, done })
            .await?;
        rx.await?
    }

    pub async fn forward(
        &self,
        aid: AgentId,
        event: ExternalEvent,
    ) -> Result<()> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::Forward { aid, event, done })
            .await?;
        rx.await?
    }

    pub async fn allocate_agent_id(&self) -> Result<AgentId> {
        let (done, rx) = oneshot::channel();
        self.tx.send(RouterCommand::Allocate { done }).await?;
        Ok(rx.await?)
    }

    pub async fn status(
        &self,
        aid: AgentId,
        ping: StatusPing,
    ) -> Result<()> {
        self.tx.send(RouterCommand::Status { aid, ping }).await?;
        Ok(())
    }

    pub async fn attach_runtime(
        &self,
        aid: AgentId,
        runtime: RuntimeHandle,
    ) -> Result<()> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::AttachRuntime { aid, runtime, done })
            .await?;
        rx.await?
    }

    pub async fn runtime_down(
        &self,
        aid: AgentId,
        error: String,
    ) -> Result<()> {
        self.tx
            .send(RouterCommand::RuntimeDown { aid, error })
            .await?;
        Ok(())
    }

    /// registers the child synchronously; resolves once the {graph record,
    /// history, workdir} capture is durable
    pub async fn spawn_agent(
        &self,
        parent: AgentId,
        capture: History,
        prompt: String,
    ) -> Result<AgentId> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::Spawn {
                parent,
                capture,
                prompt,
                done,
            })
            .await?;
        rx.await?
    }

    pub async fn send_message(
        &self,
        caller: AgentId,
        target: AgentId,
        text: String,
    ) -> Result<Result<(), RouterError>> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::Send {
                caller,
                target,
                text,
                done,
            })
            .await?;
        Ok(rx.await?)
    }

    pub async fn inspect(
        &self,
        caller: AgentId,
        target: AgentId,
    ) -> Result<Result<NodeStatus, RouterError>> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::Inspect {
                caller,
                target,
                done,
            })
            .await?;
        Ok(rx.await?)
    }

    /// suspends until the target next goes idle (or dies)
    pub async fn wait(
        &self,
        caller: AgentId,
        target: AgentId,
    ) -> Result<Result<WaitResult, RouterError>> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::Wait {
                caller,
                target,
                done,
            })
            .await?;
        Ok(rx.await?)
    }

    /// `None` = unknown caller
    pub async fn list(
        &self,
        caller: AgentId,
        subtree: bool,
    ) -> Result<Option<Vec<ListEntry>>> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::List {
                caller,
                subtree,
                done,
            })
            .await?;
        Ok(rx.await?)
    }

    /// resolves once the subtree's graph records are durably archived and its
    /// mounts released
    pub async fn archive(
        &self,
        caller: AgentId,
        target: AgentId,
    ) -> Result<Result<(), RouterError>> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::Archive {
                caller,
                target,
                done,
            })
            .await?;
        Ok(rx.await?)
    }

    pub async fn archive_tab(
        &self,
        primary: AgentId,
    ) -> Result<()> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::ArchiveTab { primary, done })
            .await?;
        rx.await?
    }

    pub async fn rollback_spawn(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(RouterCommand::RollbackSpawn { aid, done })
            .await?;
        rx.await?
    }
}

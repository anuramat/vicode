use anyhow::Result;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::agent::Agent;
use crate::agent::event::AgentEvent;
use crate::agent::event::UiEvent;
use crate::llm::history::message::UserMessage;

impl Agent {
    /// the runtime: `mail` is the node's mailbox, handed over by `launch`
    pub async fn run(
        mut self,
        mut mail: UnboundedReceiver<UserMessage>,
    ) -> Result<()> {
        match self.run_inner(&mut mail).await {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::error!("fatal error in agent {}: {:?}", self.id, e);
                drop(self.emit(UiEvent::Error(e.to_string())).await);
                Err(e)
            }
        }
    }

    async fn run_inner(
        &mut self,
        mail: &mut UnboundedReceiver<UserMessage>,
    ) -> Result<()> {
        self.project
            .mount_agent(&self.core.state.context.commit, &self.id)
            .await?;
        self.emit(UiEvent::Started {
            state: Box::new(self.core.state.clone()),
            control: self.user_tx.clone(),
        })
        .await?;
        // flush the saved buffer (a spawn seed, or messages buffered
        // before a restart) and start its turn, so the agent doesn't sit on
        // unread mail
        self.resume().await?;
        // the node leaves `Spawning`: the workdir is mounted, the state live
        self.report_status();
        while let Some(event) = self.next_event(mail).await {
            if let Err(e) = self.handle(event).await {
                tracing::error!("error in agent {}: {:?}", self.id, e);
                self.emit(UiEvent::Error(e.to_string())).await?;
            }
        }
        Ok(())
    }

    /// the multi-way event source: user control first, then the mailbox,
    /// then the tasks' streams and output (FIFO per task, so a task's events
    /// precede its terminal: the reaper is polled only once the task lane is
    /// drained)
    pub async fn next_event(
        &mut self,
        mail: &mut UnboundedReceiver<UserMessage>,
    ) -> Option<AgentEvent> {
        tokio::select! {
            biased;
            Some(command) = self.user_rx.recv() => Some(AgentEvent::User(command)),
            Some(msg) = mail.recv() => Some(AgentEvent::Message(msg)),
            Some(event) = self.task_rx.recv() => Some(event),
            Some((id, result)) = self.executor.reap() => Some(AgentEvent::Done(id, result)),
            else => None,
        }
    }
}

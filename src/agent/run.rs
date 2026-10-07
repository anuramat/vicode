use anyhow::Result;

use crate::agent::Agent;
use crate::agent::event::AgentEvent;
use crate::agent::event::UiEvent;
use crate::utils::now;

impl Agent {
    /// the runtime, started by the router once the agent's node is live
    pub async fn run(mut self) -> Result<()> {
        match self.run_inner().await {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::error!("fatal error in agent {}: {:?}", self.id, e);
                self.emit(UiEvent::Error(e.to_string()));
                Err(e)
            }
        }
    }

    async fn run_inner(&mut self) -> Result<()> {
        self.project
            .mount_agent(&self.state.commit, &self.id)
            .await?;
        self.emit(UiEvent::Started {
            state: Box::new(self.state.clone()),
            control: self.user_tx.clone(),
        });
        // flush the saved buffer (a spawn seed, or messages buffered
        // before a restart) and start its turn, so the agent doesn't sit on
        // unread mail; its step makes the startup report
        self.resume(now()).await?;
        while let Some(event) = self.next_event().await {
            if let Err(e) = self.handle(now(), event).await {
                tracing::error!("error in agent {}: {:?}", self.id, e);
                self.emit(UiEvent::Error(e.to_string()));
            }
        }
        Ok(())
    }

    /// the multi-way event source: user control first, then the event
    /// channel — the tasks' streams and output, and inter-agent mail (FIFO
    /// per task, so a task's events precede its terminal: the reaper is
    /// polled only once the channel is drained)
    pub async fn next_event(&mut self) -> Option<AgentEvent> {
        tokio::select! {
            biased;
            Some(command) = self.user_rx.recv() => Some(AgentEvent::User(command)),
            Some(event) = self.task_rx.recv() => Some(event),
            Some((id, result)) = self.executor.reap() => Some(AgentEvent::Done(id, result)),
            else => None,
        }
    }
}

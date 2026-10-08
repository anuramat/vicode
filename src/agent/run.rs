use anyhow::Result;

use crate::agent::Agent;
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
            control: self.tx.clone(),
        });
        // flush the saved buffer (a spawn seed, or messages buffered
        // before a restart) and start its turn, so the agent doesn't sit on
        // unread mail; its step makes the startup report
        self.resume(now()).await?;
        // the agent holds a sender, so this only ends when the runtime is
        // aborted; a task sends its `Done` after its events, so FIFO
        // delivers them in order
        while let Some(event) = self.rx.recv().await {
            if let Err(e) = self.handle(now(), event).await {
                tracing::error!("error in agent {}: {:?}", self.id, e);
                self.emit(UiEvent::Error(e.to_string()));
            }
        }
        Ok(())
    }
}

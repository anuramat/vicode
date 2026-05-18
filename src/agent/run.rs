use std::ops::ControlFlow;

use anyhow::Result;

use crate::agent::Agent;
use crate::agent::handle::ParentEvent;

impl Agent {
    pub async fn run(mut self) {
        if let Err(e) = self.run_inner().await {
            tracing::error!("fatal error in agent {}: {:?}", self.id, e);
            drop(self.emit(ParentEvent::Error(e.to_string())).await);
        }
    }

    async fn run_inner(&mut self) -> Result<()> {
        self.project
            .mount_agent(&self.state.context.commit, &self.id)
            .await?;
        self.emit(ParentEvent::Started(Box::new(self.state.clone())))
            .await?;
        while let Some(event) = self.rx.recv().await {
            match self.handle(event).await {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(())) => break,
                Err(e) => {
                    tracing::error!("error in agent {}: {:?}", self.id, e);
                    self.emit(ParentEvent::Error(e.to_string())).await?;
                }
            }
        }
        Ok(())
    }
}

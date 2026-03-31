use std::ops::ControlFlow;

use anyhow::Result;
use futures::stream::AbortHandle;

use crate::agent::Agent;
use crate::agent::AgentHandle;
use crate::agent::AgentStatus;
use crate::agent::handle::ParentEvent;
use crate::project::PROJECT;

impl Agent {
    pub async fn run(
        mut self,
        abort: AbortHandle,
    ) -> Result<()> {
        PROJECT
            .mount_agent(&self.state.context.commit, &self.id)
            .await?;
        self.parent
            .send(ParentEvent::Started(
                AgentHandle {
                    tx: self.tx.clone(),
                    state: self.state.clone(),
                    abort,
                }
                .into(),
            ))
            .await?;
        while let Some(event) = self.rx.recv().await {
            match self.handle(event).await {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(())) => break,
                Err(e) => {
                    tracing::error!("error in agent {}: {:?}", self.id, e);
                    self.parent.send(ParentEvent::Error(e.to_string())).await?;
                    self.set_status(AgentStatus::Error(e.to_string())).await?;
                }
            }
        }
        Ok(())
    }
}

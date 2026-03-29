use std::ops::ControlFlow;

use anyhow::Result;
use futures::stream::AbortHandle;

use crate::agent::Agent;
use crate::agent::AgentHandle;
use crate::agent::handle::AgentStarted;
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
            .send(ParentEvent::Started(AgentStarted {
                aid: self.id.clone(),
                state: self.state.clone(),
                handle: AgentHandle {
                    tx: self.tx.clone(),
                    abort,
                },
            }))
            .await?;
        self.parent.send(ParentEvent::InfoUpdate).await?;
        while let Some(event) = self.rx.recv().await {
            match self.handle(event).await {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(())) => break,
                Err(e) => {
                    let msg = e.to_string();
                    self.parent.send(ParentEvent::Error(msg)).await?;
                }
            }
        }
        Ok(())
    }
}

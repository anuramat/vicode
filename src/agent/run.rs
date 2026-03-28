use anyhow::Result;

use crate::agent::Agent;
use crate::agent::handle::ParentEvent;
use crate::project::PROJECT;

impl Agent {
    pub async fn run(mut self) -> Result<()> {
        PROJECT.mount(&self.state.context.commit, &self.id).await?;
        self.parent.send(ParentEvent::InfoUpdate).await?;
        while let Some(event) = self.rx.recv().await {
            if let Err(e) = self.handle(event).await {
                let msg = e.to_string();
                self.parent.send(ParentEvent::Error(msg)).await?;
            }
        }
        Ok(())
    }
}

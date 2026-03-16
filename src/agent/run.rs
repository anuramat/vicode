use anyhow::Result;

use crate::agent::Agent;
use crate::agent::handle::ParentEvent;
use crate::project::PROJECT;

impl Agent {
    pub async fn run(mut self) -> Result<()> {
        PROJECT.mount(&self.state.context.commit, &self.id).await?;
        self.parent
            .send(ParentEvent::InfoUpdate(self.id.clone()))
            .await?;
        while let Some(event) = self.rx.recv().await {
            self.handle(event).await?;
        }
        Ok(())
    }
}

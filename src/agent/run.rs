use anyhow::Result;

use crate::agent::Agent;
use crate::agent::event::UiEvent;
use crate::utils::now;

impl Agent {
    /// a failure is reported by `Router::runtime_down`
    pub async fn run(mut self) -> Result<()> {
        self.project
            .mount_agent(&self.state.commit, &self.id)
            .await?;
        self.emit(UiEvent::Started {
            state: Box::new(self.state.clone()),
            control: self.tx.clone(),
        });
        while let Some(event) = self.rx.recv().await {
            if let Err(e) = self.handle(now(), event).await {
                self.report_error(e);
            }
        }
        Ok(())
    }
}

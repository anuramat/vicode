use anyhow::Result;

use crate::agent::Agent;
use crate::agent::handle::ParentEvent;
use crate::agent::task::manager::TaskId;

impl Agent {
    pub async fn handle_task_result(
        &mut self,
        id: TaskId,
        event: Result<()>,
    ) -> Result<()> {
        let finished = self.apply_task_result(id, event).await?;
        if !finished {
            return Ok(());
        }
        if self.tskmgr.idle() {
            if self.state.context.history.needs_another_turn() {
                self.start_turn();
            } else {
                self.parent.send(ParentEvent::TurnComplete).await?;
            }
        }
        self.parent.send(ParentEvent::InfoUpdate).await?;
        Ok(())
    }

    async fn apply_task_result(
        &mut self,
        id: TaskId,
        event: Result<()>,
    ) -> Result<bool> {
        if !self.tskmgr.pending(&id) {
            return Ok(false);
        }
        match event {
            Ok(()) => {
                self.tskmgr.finish_task(&id)?;
                Ok(true)
            }
            Err(err) => {
                self.parent
                    .send(ParentEvent::Error(err.to_string()))
                    .await?;
                self.tskmgr.finish_task(&id)?;
                Ok(true)
            }
        }
    }
}

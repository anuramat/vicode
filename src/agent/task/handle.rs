use anyhow::Result;

use crate::agent::Agent;
use crate::agent::handle::ParentEvent;
use crate::agent::task::TaskEvent;
use crate::agent::task::manager::TaskId;

impl Agent {
    pub async fn handle_task_event(
        &mut self,
        id: TaskId,
        event: TaskEvent,
    ) -> Result<()> {
        let finished = self.apply_task_event(id, event).await?;
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

    async fn apply_task_event(
        &mut self,
        id: TaskId,
        event: TaskEvent,
    ) -> Result<bool> {
        if !self.tskmgr.pending(&id) {
            return Ok(false);
        }
        match event {
            TaskEvent::Delta(update) => {
                update.apply(self).await?;
                Ok(false)
            }
            TaskEvent::Result(result) => {
                result.apply(self).await?;
                self.tskmgr.finish_task(&id)?;
                Ok(true)
            }
            TaskEvent::Error(msg) => {
                self.parent.send(ParentEvent::Error(msg)).await?;
                self.tskmgr.finish_task(&id)?;
                Ok(true)
            }
        }
    }
}

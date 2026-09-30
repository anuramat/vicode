use anyhow::Result;

use crate::agent::Agent;
use crate::agent::event::AgentEvent;
use crate::agent::event::UiEvent;

impl Agent {
    pub async fn run(mut self) -> Result<()> {
        match self.run_inner().await {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::error!("fatal error in agent {}: {:?}", self.id, e);
                drop(self.emit(UiEvent::Error(e.to_string())).await);
                Err(e)
            }
        }
    }

    async fn run_inner(&mut self) -> Result<()> {
        self.project
            .mount_agent(&self.core.state.context.commit, &self.id)
            .await?;
        self.emit(UiEvent::Started(Box::new(self.core.state.clone())))
            .await?;
        // flush messages buffered before a restart and start their
        // turn, so a restored agent doesn't sit on unread mail
        self.resume().await?;
        // unconditional startup report: with parked deliveries outstanding the
        // router sees `processed < delivered` and keeps the node woken, so an
        // idle startup reports in without firing waits stale
        self.report_status(None).await?;
        while let Some(event) = self.next_event().await {
            // router deliveries are seq-counted; count before handling so
            // every report from here on carries the new watermark
            let counted = matches!(event, AgentEvent::Inbound(_) | AgentEvent::User(_));
            if counted {
                self.processed += 1;
            }
            let error = self.handle(event).await.err();
            if let Some(e) = &error {
                tracing::error!("error in agent {}: {:?}", self.id, e);
                self.emit(UiEvent::Error(e.to_string())).await?;
            }
            if counted {
                // every delivery reports — success or handler error — so a
                // wake that starts no turn can't strand waiters
                self.report_status(error.map(|e| e.to_string())).await?;
            }
        }
        Ok(())
    }

    /// the multi-way event source: user control first, then the mailbox,
    /// then the tasks' streams and output (FIFO per task, so a task's events
    /// precede its terminal: the reaper is polled only once the task lane is
    /// drained)
    pub async fn next_event(&mut self) -> Option<AgentEvent> {
        tokio::select! {
            biased;
            Some(event) = self.user_rx.recv() => Some(event),
            Some(event) = self.rx.recv() => Some(event),
            Some(event) = self.task_rx.recv() => Some(event),
            Some((id, result)) = self.executor.reap() => Some(AgentEvent::Done(id, result)),
            else => None,
        }
    }
}

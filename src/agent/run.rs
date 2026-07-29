use anyhow::Result;
use anyhow::anyhow;
use tokio::task::JoinError;

use crate::agent::Agent;
use crate::agent::core::ABORTED_BY_USER;
use crate::agent::handle::AgentEvent;
use crate::agent::handle::ParentEvent;
use crate::agent::task::executor::TaskMeta;
use crate::agent::task::executor::TaskOutput;
use crate::tui::app::AppEvent;

impl Agent {
    pub async fn run(mut self) -> Result<()> {
        match self.run_inner().await {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::error!("fatal error in agent {}: {:?}", self.id, e);
                drop(self.emit(ParentEvent::Error(e.to_string())).await);
                Err(e)
            }
        }
    }

    async fn run_inner(&mut self) -> Result<()> {
        self.project
            .mount_agent(&self.core.state.context.commit, &self.id)
            .await?;
        self.emit(ParentEvent::Started(Box::new(self.core.state.clone())))
            .await?;
        // flush messages buffered before a restart and start their
        // turn, so a restored agent doesn't sit on unread mail (M3, §2.2)
        self.resume().await?;
        // unconditional startup ping: with parked deliveries outstanding the
        // router sees `processed < delivered` and keeps the node woken, so an
        // idle startup reports in without firing waits stale (H4)
        self.ping_status(None).await?;
        while let Some(event) = self.next_event().await {
            // router deliveries are seq-counted; count before handling so
            // every ping from here on carries the new watermark
            let counted = matches!(event, AgentEvent::Inbound(_) | AgentEvent::External(_));
            if counted {
                self.processed += 1;
            }
            let error = self.handle(event).await.err();
            if let Some(e) = &error {
                tracing::error!("error in agent {}: {:?}", self.id, e);
                self.emit(ParentEvent::Error(e.to_string())).await?;
            }
            if counted {
                // every delivery pings — success or handler error — so a
                // wake that starts no turn can't strand waiters (H4)
                self.ping_status(error.map(|e| e.to_string())).await?;
            }
        }
        Ok(())
    }

    /// the multi-way event source: user control first, then the mailbox, then
    /// output chunks (processed inline — they never become core events; ranked
    /// below `rx` so a firehose tool can't starve turn stream events and
    /// inbound messages, M9), and the executor reaper last, so a turn's queued
    /// stream events land before its terminal
    pub async fn next_event(&mut self) -> Option<AgentEvent> {
        loop {
            tokio::select! {
                biased;
                Some(event) = self.user_rx.recv() => return Some(event),
                Some(event) = self.rx.recv() => return Some(event),
                Some((call_id, chunk)) = self.out_rx.recv() => self.on_chunk(call_id, chunk),
                Some((meta, output)) = self.executor.reap() => {
                    if let Some(event) = self.reap_event(meta, output) {
                        return Some(event);
                    }
                }
                else => return None,
            }
        }
    }

    /// tee: accumulate (authoritative) + forward to the app for live render
    /// (droppable — a full app bus costs render frames, never stalls us)
    pub fn on_chunk(
        &mut self,
        call_id: String,
        chunk: String,
    ) {
        self.accumulators
            .entry(call_id.clone())
            .or_default()
            .push_str(&chunk);
        drop(self.router.app_tx().try_send(AppEvent::ParentEvent(
            self.id.clone(),
            ParentEvent::ToolOutput { call_id, chunk },
        )));
    }

    /// the single resolver (§2.4a): turn a task termination into its one
    /// terminal event; None only for a cancelled turn, whose resolution
    /// (failed response, cleared ledger) already happened in `abort`
    pub fn reap_event(
        &mut self,
        meta: TaskMeta,
        output: Result<TaskOutput, JoinError>,
    ) -> Option<AgentEvent> {
        // flush chunks already in flight so a finalize sees them
        while let Ok((call_id, chunk)) = self.out_rx.try_recv() {
            self.on_chunk(call_id, chunk);
        }
        match output {
            Ok(TaskOutput::Turn(result)) => Some(AgentEvent::TaskDone(meta.id, result)),
            Ok(TaskOutput::Tool(mut item)) => {
                // compose (§2.4a): a streaming tool's authoritative text is
                // the accumulator; its return carries only metadata
                if let Some(streamed) = self.accumulators.remove(&item.call_id) {
                    item.task.compose(streamed);
                }
                Some(AgentEvent::ToolResolved(meta.id, item))
            }
            Err(e) => {
                let Some(call_id) = meta.call_id else {
                    return e.is_panic().then(|| {
                        AgentEvent::TaskDone(meta.id, Err(anyhow!("turn panicked: {e}")))
                    });
                };
                let marker = if e.is_cancelled() {
                    ABORTED_BY_USER.to_string()
                } else {
                    // extract the payload: JoinError's display embeds an
                    // unstable task id
                    match e.try_into_panic() {
                        Ok(payload) => format!("tool panicked: {}", panic_message(&*payload)),
                        Err(e) => format!("tool failed: {e}"),
                    }
                };
                let error = match self.accumulators.remove(&call_id) {
                    Some(partial) if !partial.is_empty() => {
                        format!("{marker}; partial output:\n{partial}")
                    }
                    _ => marker,
                };
                Some(AgentEvent::ToolFailed {
                    id: meta.id,
                    call_id,
                    error,
                })
            }
        }
    }
}

pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

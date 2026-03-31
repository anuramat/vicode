use anyhow::Result;
use futures::StreamExt;
use tracing::instrument;
use tracing::trace;

use super::*;
use crate::agent::task::sink::TaskHandle;
use crate::llm::history::HistoryEvent;
use crate::llm::provider::api::StreamEvent;

// TODO should these ResponseFailed events also coincide with ParentEvent::Error? and if so, should we emit ParentEvent::Error right here or in the HistoryEvent handler in the agent event loop?

impl Agent {
    pub fn start_turn(&mut self) {
        let instructions = self.state.context.instructions.clone();
        let history = self.state.context.history.clone();
        let generation = self.state.context.history.generation();

        tracing::debug!("starting turn with messages: {:#?}", history);
        let assistant = self.assistant.clone();
        let tools = self.tools.clone();
        self.tskmgr
            .spawn(self.tx.clone(), generation, move |task| async move {
                Agent::turn(task, &assistant, tools, instructions, history).await
            });
    }

    #[instrument(skip(task, instructions, history, assistant, tools))]
    pub async fn turn(
        task: TaskHandle,
        assistant: &Assistant,
        tools: ToolSchemas,
        instructions: String,
        history: History,
    ) -> Result<()> {
        let started = assistant.stream_turn(instructions, history, tools).await?;
        task.history(HistoryEvent::ResponseStarted(started.started_at_ms))
            .await?;

        let mut stream = started.stream;
        while let Some(event) = stream.next().await {
            trace!(event = ?event, "Stream chunk received");
            match event? {
                StreamEvent::Delta(delta) => {
                    task.history(HistoryEvent::ResponseDelta(delta)).await?
                }
                StreamEvent::Failed(msg) => {
                    let event = HistoryEvent::ResponseFailed(msg);
                    task.history(event).await?;
                }
                StreamEvent::ItemDone(mut item) => {
                    item.timing_mut().touch();
                    task.history(HistoryEvent::ResponseItem(item.into()))
                        .await?;
                }
                StreamEvent::ItemAdded(item) => {
                    task.history(HistoryEvent::ResponseItem(item.into()))
                        .await?;
                }
                StreamEvent::Completed(items) => {
                    task.history(HistoryEvent::ResponseCompleted(items)).await?;
                    break;
                }
                StreamEvent::Ignore => {}
            }
        }
        Ok(())
    }
}

use anyhow::Result;
use futures::StreamExt;
use tokio::sync::mpsc::Sender;
use tracing::instrument;
use tracing::trace;

use super::*;
use crate::llm::delta::*;
use crate::llm::history::HistoryEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::message::AssistantItem;
use crate::llm::provider::api::StreamEvent;

async fn send_item(
    tx: &Sender<AgentEvent>,
    generation: HistoryGeneration,
    item: AssistantItem,
) -> Result<()> {
    Ok(tx
        .send(AgentEvent::HistoryEvent(
            generation,
            HistoryEvent::ResponseItem(Box::new(item)),
        ))
        .await?)
}

async fn send_completed(
    tx: &Sender<AgentEvent>,
    generation: HistoryGeneration,
    items: Vec<AssistantItem>,
) -> Result<()> {
    Ok(tx
        .send(AgentEvent::HistoryEvent(
            generation,
            HistoryEvent::ResponseCompleted(items),
        ))
        .await?)
}

async fn send_delta(
    tx: &Sender<AgentEvent>,
    generation: HistoryGeneration,
    delta: Delta,
) -> Result<()> {
    Ok(tx
        .send(AgentEvent::HistoryEvent(
            generation,
            HistoryEvent::ResponseDelta(delta),
        ))
        .await?)
}

async fn send_started(
    tx: &Sender<AgentEvent>,
    generation: HistoryGeneration,
    started_at_ms: u64,
) -> Result<()> {
    Ok(tx
        .send(AgentEvent::HistoryEvent(
            generation,
            HistoryEvent::ResponseStarted(started_at_ms),
        ))
        .await?)
}

// TODO should these ResponseFailed events also coincide with ParentEvent::Error? and if so, should
// we emit ParentEvent::Error right here or in the HistoryEvent handler in the agent event loop?

impl Agent {
    pub fn start_turn(&mut self) {
        let instructions = self.state.context.instructions.clone();
        let history = self.state.context.history.clone();
        let generation = history.generation();

        tracing::debug!("starting turn with messages: {:#?}", history);
        let tx = self.tx.clone();
        let assistant = self.assistant.clone();
        let tools = self.tools.clone();
        self.tskmgr.spawn(self.tx.clone(), move |task| async move {
            let res = Agent::turn(tx.clone(), &assistant, tools, instructions, history).await;
            if let Err(e) = res {
                let event = HistoryEvent::ResponseFailed(e.to_string());
                tx.send(AgentEvent::HistoryEvent(generation, event))
                    .await
                    .expect("failed to send turn error event");
            }
            task.result(()).await
        });
    }

    #[instrument(skip(instructions, history, assistant, tools))]
    pub async fn turn(
        tx: Sender<AgentEvent>,
        assistant: &Assistant,
        tools: ToolSchemas,
        instructions: String,
        history: History,
    ) -> Result<()> {
        let generation = history.generation();
        let started = assistant.stream_turn(instructions, history, tools).await?;
        send_started(&tx, generation, started.started_at_ms).await?;
        let mut stream = started.stream;
        let generation = generation + 1;

        while let Some(event) = stream.next().await {
            trace!(event = ?event, "Stream chunk received");

            match event? {
                StreamEvent::Delta(delta) => send_delta(&tx, generation, delta).await?,
                StreamEvent::Failed(msg) => {
                    let event = HistoryEvent::ResponseFailed(msg);
                    tx.send(AgentEvent::HistoryEvent(generation, event)).await?;
                    break;
                }
                StreamEvent::ItemDone(mut item) => {
                    item.timing_mut().touch();
                    send_item(&tx, generation, item).await?;
                }
                StreamEvent::ItemAdded(item) => {
                    send_item(&tx, generation, item).await?;
                }
                StreamEvent::Completed(items) => {
                    send_completed(&tx, generation, items).await?;
                    break;
                }
                StreamEvent::Ignore => {}
            }
        }
        Ok(())
    }
}

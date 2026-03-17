use anyhow::Result;
use futures::StreamExt;
use tokio::sync::mpsc::Sender;
use tracing::instrument;
use tracing::trace;

use super::*;
use crate::llm::provider::event::StreamEvent;
use crate::llm::delta::*;
use crate::llm::history::HistoryEvent;
use crate::llm::message::AssistantItem;

async fn send_item(
    tx: &Sender<AgentEvent>,
    loc: usize,
    item: Box<AssistantItem>,
) -> Result<()> {
    Ok(tx
        .send(AgentEvent::HistoryEvent(HistoryEvent::ResponseItem(
            loc, item,
        )))
        .await?)
}

async fn send_delta(
    tx: &Sender<AgentEvent>,
    loc: usize,
    delta: Delta,
) -> Result<()> {
    Ok(tx
        .send(AgentEvent::HistoryEvent(HistoryEvent::ResponseDelta(
            loc, delta,
        )))
        .await?)
}

impl Agent {
    pub fn start_turn(&mut self) {
        let instructions = self.state.context.instructions.clone();
        let history = self.state.context.history.clone();

        tracing::debug!("starting turn with messages: {:#?}", history);
        let tx = self.tx.clone();
        let assistant = self.assistant.clone();
        let tools = self.tools.clone();
        self.tskmgr.spawn(self.tx.clone(), async move {
            let res = Agent::turn(tx.clone(), &assistant, tools, instructions, history).await;
            if let Err(e) = res {
                tx.send(AgentEvent::TurnError(e.to_string()))
                    .await
                    .expect("failed to send turn error event");
            };
            TaskResult::AssistantResponse
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
        let loc = history.len();
        let mut stream = assistant.stream_turn(instructions, history, tools).await?;

        while let Some(event) = stream.next().await {
            trace!(event = ?event, "Stream chunk received");

            match event? {
                StreamEvent::Delta(delta) => send_delta(&tx, loc, delta).await?,
                StreamEvent::Failed(msg) => {
                    tx.send(AgentEvent::ResponseFailed(msg)).await?;
                    break;
                }
                StreamEvent::ItemDone(item) | StreamEvent::ItemAdded(item) => {
                    send_item(&tx, loc, Box::new(item)).await?;
                }
                StreamEvent::Completed(_) => break,
                StreamEvent::Ignore => {}
            }
        }
        Ok(())
    }
}

use anyhow::Result;
use futures::StreamExt;
use tokio::sync::mpsc::Sender;
use tracing::instrument;
use tracing::trace;

use super::*;
use crate::llm::delta::*;
use crate::llm::history::HistoryEvent;
use crate::llm::message::AssistantItem;
use crate::llm::provider::event::StreamEvent;

async fn send_item(
    tx: &Sender<AgentEvent>,
    loc: usize,
    item: AssistantItem,
) -> Result<()> {
    Ok(tx
        .send(AgentEvent::HistoryEvent(HistoryEvent::ResponseItem(
            loc,
            Box::new(item),
        )))
        .await?)
}

async fn send_completed(
    tx: &Sender<AgentEvent>,
    loc: usize,
    items: Vec<AssistantItem>,
) -> Result<()> {
    Ok(tx
        .send(AgentEvent::HistoryEvent(HistoryEvent::ResponseCompleted(
            loc, items,
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

// TODO should these ResponseFailed events also coincide with ParentEvent::Error? and if so, should
// we emit ParentEvent::Error right here or in the HistoryEvent handler in the agent event loop?

impl Agent {
    pub fn start_turn(&mut self) {
        let instructions = self.state.context.instructions.clone();
        let history = self.state.context.history.clone();
        let loc = history.len();

        tracing::debug!("starting turn with messages: {:#?}", history);
        let tx = self.tx.clone();
        let assistant = self.assistant.clone();
        let tools = self.tools.clone();
        self.tskmgr.spawn(self.tx.clone(), async move {
            let res = Agent::turn(tx.clone(), &assistant, tools, instructions, history).await;
            if let Err(e) = res {
                let event = HistoryEvent::ResponseFailed(loc, e.to_string());
                tx.send(AgentEvent::HistoryEvent(event))
                    .await
                    .expect("failed to send turn error event");
            }
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
                    let event = HistoryEvent::ResponseFailed(loc, msg);
                    tx.send(AgentEvent::HistoryEvent(event)).await?;
                    break;
                }
                StreamEvent::ItemDone(mut item) => {
                    item.finish();
                    send_item(&tx, loc, item).await?;
                }
                StreamEvent::ItemAdded(item) => {
                    send_item(&tx, loc, item).await?;
                }
                StreamEvent::Completed(items) => {
                    send_completed(&tx, loc, items).await?;
                    break;
                }
                StreamEvent::Ignore => {}
            }
        }
        Ok(())
    }
}

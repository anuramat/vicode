use anyhow::Result;
use futures::StreamExt;
use tracing::instrument;
use tracing::trace;

use super::*;
use crate::agent::task::sink::TurnHandle;
use crate::agent::task::sink::TurnType;
use crate::llm::history::ResponseEvent;
use crate::llm::message::Message;
use crate::llm::provider::api::StreamEvent;

// TODO should these ResponseFailed events also coincide with ParentEvent::Error? and if so, should we emit ParentEvent::Error right here or in the HistoryEvent handler in the agent event loop?

impl Agent {
    pub fn start_turn(&mut self) {
        let messages: Vec<Message> = self.state.context.history.iter().collect();

        tracing::debug!("starting turn with messages: {:#?}", messages);
        let tools = self.tools.clone();
        self.spawn_turn(
            tools,
            self.state.context.history.instructions().to_string(),
            messages,
            TurnType::Default,
        );
    }

    #[instrument(skip(self, tools, instructions, messages))]
    pub fn spawn_turn(
        &mut self,
        tools: ToolSchemas,
        instructions: String,
        messages: Vec<Message>,
        turn_type: TurnType,
    ) {
        let generation = self.state.context.history.generation();
        let assistant = self.state.assistant.clone();
        self.tskmgr
            .spawn(self.tx.clone(), generation, move |task| async move {
                let handle = TurnHandle { task, turn_type };
                if let Err(err) =
                    Agent::turn(handle.clone(), &assistant, tools, instructions, messages).await
                {
                    handle.send(ResponseEvent::Failed(err.to_string())).await?;
                    return Err(err);
                }
                Ok(())
            });
    }

    #[instrument(skip(handle, instructions, messages, assistant, tools))]
    pub async fn turn(
        handle: TurnHandle,
        assistant: &Assistant,
        tools: ToolSchemas,
        instructions: String,
        messages: Vec<Message>,
    ) -> Result<()> {
        let started = assistant.stream_turn(instructions, messages, tools).await?;
        handle
            .send(ResponseEvent::Started(started.started_at_ms))
            .await?;

        let mut stream = started.stream;
        while let Some(event) = stream.next().await {
            trace!(event = ?event, "Stream chunk received");
            match event? {
                StreamEvent::Delta(delta) => handle.send(ResponseEvent::Delta(delta)).await?,
                StreamEvent::Failed(msg) => anyhow::bail!(msg),
                StreamEvent::ItemDone(mut item) => {
                    item.timing_mut().touch();
                    handle.send(ResponseEvent::Item(item.into())).await?;
                }
                StreamEvent::ItemAdded(item) => {
                    handle.send(ResponseEvent::Item(item.into())).await?;
                }
                StreamEvent::Completed(items) => {
                    handle.send(ResponseEvent::Completed(items)).await?;
                    break;
                    // TODO try dropping the break
                }
                StreamEvent::Ignore => {}
            }
        }
        Ok(())
    }
}

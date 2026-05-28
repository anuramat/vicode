use anyhow::Result;
use futures::StreamExt;
use tracing::instrument;
use tracing::trace;

use super::Agent;
use super::Assistant;
use super::ToolRegistry;
use crate::agent::task::sink::TurnHandle;
use crate::agent::task::sink::TurnType;
use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::Message;
use crate::llm::provider::api::StreamEvent;

// TODO should these ResponseFailed events also coincide with ParentEvent::Error? and if so, should we emit ParentEvent::Error right here or in the HistoryEvent handler in the agent event loop?

impl Agent {
    pub async fn start_turn(&mut self) -> Result<()> {
        let messages: Vec<Message> = self.history().state().messages.clone();

        tracing::debug!("starting turn with messages: {:#?}", messages);
        let tools = self.tools.clone();
        self.spawn_turn(
            tools,
            self.history().instructions().to_string(),
            messages,
            TurnType::Default,
        )
        .await
    }

    #[instrument(skip(self, tools, instructions, messages))]
    pub async fn spawn_turn(
        &mut self,
        tools: ToolRegistry,
        instructions: String,
        messages: Vec<Message>,
        turn_type: TurnType,
    ) -> Result<()> {
        let created = match turn_type {
            TurnType::Default => HistoryUpdate::TurnResponse(AssistantEvent::created()),
            TurnType::Compact => HistoryUpdate::CompactResponse(AssistantEvent::created()),
        };
        let generation = self.history().generation();
        self.handle_history(generation, created).await?;

        let assistant = self.state.assistant.clone();
        self.tskmgr
            .spawn(self.tx.clone(), generation, move |task| async move {
                let handle = TurnHandle { task, turn_type };
                if let Err(err) =
                    Self::turn(handle.clone(), &assistant, tools, instructions, messages).await
                {
                    handle.send(AssistantEvent::Failed(err.to_string())).await?;
                    return Err(err);
                }
                Ok(())
            });
        Ok(())
    }

    #[instrument(skip(handle, instructions, messages, assistant, tools))]
    pub async fn turn(
        handle: TurnHandle,
        assistant: &Assistant,
        tools: ToolRegistry,
        instructions: String,
        messages: Vec<Message>,
    ) -> Result<()> {
        let started = assistant.stream_turn(instructions, messages, tools).await?;
        handle
            .send(AssistantEvent::Started(started.started_at_ms))
            .await?;
        let mut stream = started.stream;
        while let Some(event) = stream.next().await {
            trace!(event = ?event, "Stream chunk received");
            match event? {
                StreamEvent::Delta(delta) => handle.send(AssistantEvent::Delta(delta)).await?,
                StreamEvent::Failed(msg) => anyhow::bail!(msg),
                StreamEvent::ItemDone(mut item) => {
                    item.touch_ended_at_now();
                    handle.send(AssistantEvent::Item(item.into())).await?;
                }
                StreamEvent::ItemAdded(item) => {
                    handle.send(AssistantEvent::Item(item.into())).await?;
                }
                StreamEvent::Completed(items) => {
                    handle.send(AssistantEvent::Completed(items)).await?;
                    break;
                    // TODO try dropping the break
                }
                StreamEvent::Ignore => {}
            }
        }
        Ok(())
    }
}

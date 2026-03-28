use anyhow::Context;
use anyhow::Result;
use futures::StreamExt;

use crate::agent::Agent;
use crate::agent::handle::ParentEvent;
use crate::agent::task::TaskDelta;
use crate::agent::task::TaskResult;
use crate::agent::tool::registry::ToolSchemas;
use crate::config::CONFIG;
use crate::llm::delta::DeltaContent;
use crate::llm::history::History;
use crate::llm::provider::api::StreamEvent;

const COMPACT_PROMPT: &str = "Summarize this conversation for future continuation. Keep concrete user requirements, decisions, constraints, file paths, and unresolved work. Be concise and factual. Output plain text only.";

#[derive(Debug)]
struct CompactDelta(String);

#[async_trait::async_trait]
impl TaskDelta for CompactDelta {
    async fn apply(
        self: Box<Self>,
        agent: &mut Agent,
    ) -> Result<()> {
        agent.state.context.history.push_compact_delta(self.0);
        Ok(())
    }
}

#[derive(Debug)]
struct CompactResult;

#[async_trait::async_trait]
impl TaskResult for CompactResult {
    async fn apply(
        self: Box<Self>,
        agent: &mut Agent,
    ) -> Result<()> {
        agent.state.context.history.finish_compact()?;
        agent.save().await?;
        agent
            .parent
            .send(ParentEvent::HistoryReset(
                agent.state.context.history.clone(),
            ))
            .await
    }
}

impl Agent {
    pub fn start_compact(&mut self) -> Result<()> {
        if self.state.context.history.compacting().is_some() {
            return Ok(());
        }
        let window = self
            .assistant
            .config
            .model
            .window
            .context("compact requires assistant.model.window")?;
        let history = &self.state.context.history;
        if history.total_tokens() * 100 < CONFIG.compact.threshold * window {
            return Ok(());
        }
        let dropped = history.compact_dropped(window, CONFIG.compact.target);
        if dropped == 0 {
            return Ok(());
        }
        let history = History::from_messages(history.compactable_messages(dropped));
        let assistant = self.assistant.clone();
        self.state.context.history.start_compact(dropped);
        self.tskmgr.spawn(self.tx.clone(), move |task| async move {
            let mut stream = assistant
                .stream_turn(COMPACT_PROMPT.into(), history, ToolSchemas(Vec::new()))
                .await?
                .stream;
            while let Some(event) = stream.next().await {
                match event? {
                    StreamEvent::Delta(delta) => {
                        if let DeltaContent::Output(text) = delta.delta {
                            task.delta(CompactDelta(text)).await?;
                        }
                    }
                    StreamEvent::Completed(_) => {
                        task.result(CompactResult).await?;
                        break;
                    }
                    StreamEvent::Failed(msg) => {
                        task.error(msg).await?;
                        break;
                    }
                    StreamEvent::Ignore | StreamEvent::ItemAdded(_) | StreamEvent::ItemDone(_) => {}
                }
            }
            Ok(())
        });
        Ok(())
    }
}

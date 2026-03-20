use anyhow::Result;
use futures::future::try_join_all;
use tracing::debug;
use tracing::error;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::TaskId;
use crate::agent::TaskResult;
use crate::agent::id::AgentId;
use crate::agent::replica;
use crate::llm::history;
use crate::llm::history::HistoryEvent;
use crate::llm::history::HistoryLoc;
use crate::llm::message::AssistantItem;
use crate::llm::message::now_ms;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::project::PROJECT;

#[derive(Debug)]
pub enum AgentEvent {
    TaskDone(TaskId, TaskResult),
    Submit(UserPrompt),
    SetAssistant(String),
    HistoryEvent(HistoryLoc, HistoryEvent),
    /// delete agent, e.g. when deleting a tab
    Delete,
    DuplicateRequest(AgentId),
}

// TODO drop
pub type ParentMessage = (AgentId, ParentEvent);

#[derive(Debug)]
pub enum ParentEvent {
    AttachAgent,
    InfoUpdate,
    HistoryUpdate(HistoryLoc, HistoryEvent),
    TurnComplete,
    Error(String),
}

#[derive(Debug)]
pub struct UserPrompt {
    pub text: Option<String>,
    pub multiplier: usize,
    pub loc: usize,
}

impl Agent {
    #[instrument(skip(self))]
    pub async fn handle(
        &mut self,
        event: AgentEvent,
    ) -> Result<()> {
        use AgentEvent::*;

        debug!(event = ?event, "handling agent event");

        match event {
            TaskDone(loc, result) => {
                self.apply_task_result(loc, result).await?;
                if self.tskmgr.pending.is_empty() {
                    if self.state.context.history.needs_another_turn() {
                        self.start_turn();
                    } else {
                        self.parent
                            .send((self.id.clone(), ParentEvent::TurnComplete))
                            .await?;
                    }
                }
                self.parent
                    .send((self.id.clone(), ParentEvent::InfoUpdate))
                    .await?;
            }
            DuplicateRequest(aid) => {
                self.try_duplicate(aid).await?;
            }
            Submit(UserPrompt {
                text,
                multiplier,
                loc,
            }) => {
                if let Some(text) = text {
                    self.handle_history(loc, history::HistoryEvent::UserMessage(text))
                        .await?;
                }
                if self.tskmgr.pending.is_empty() {
                    if multiplier <= 1 {
                        self.start_turn();
                    } else {
                        // TODO insert a developer message <n replicas pending>
                        let replicas =
                            try_join_all((0..multiplier).map(|_| AgentId::new())).await?;
                        self.state.topology.children.extend(replicas.clone());
                        self.save().await?;
                        self.start_replica_turns(replicas);
                    }
                }
            }
            SetAssistant(id) => {
                self.set_assistant(&id).await?;
            }
            HistoryEvent(loc, event) => {
                self.handle_history(loc, event).await?;
            }
            Delete => {
                PROJECT.delete_agent(&self.id).await?;
            }
        }
        Ok(())
    }

    pub async fn handle_history(
        &mut self,
        loc: HistoryLoc,
        event: HistoryEvent,
    ) -> Result<()> {
        // TODO verify
        self.parent
            .send((
                self.id.clone(),
                ParentEvent::HistoryUpdate(loc, event.clone()),
            ))
            .await?;
        self.state.context.history.handle(loc, event.clone());
        match event {
            HistoryEvent::ResponseStarted(_) => {}
            HistoryEvent::ResponseItem(ref item) => {
                if let AssistantItem::ToolCall(mut call) = (**item).clone()
                    && call.task.output().is_none()
                {
                    call.task.prepare(self)?;
                    self.tskmgr.spawn(self.tx.clone(), async move {
                        call.task.run().await;
                        call.executed_at_ms = Some(now_ms());
                        TaskResult::ToolCall(loc, call)
                    });
                }
            }
            HistoryEvent::ResponseFailed(msg) => {
                error!("error in agent {}: {}", self.id, msg);
            }
            _ => {}
        }

        self.save().await?;
        Ok(())
    }

    fn start_replica_turns(
        &mut self,
        replicas: Vec<AgentId>,
    ) {
        let parent = self.id.clone();
        let context = self.state.context.clone();
        self.tskmgr.spawn(self.tx.clone(), async move {
            // TODO error handling
            TaskResult::ReplicaRun(
                replica::run_replicas(parent, context, replicas)
                    .await
                    .unwrap(),
            )
        });
    }

    pub async fn set_assistant(
        &mut self,
        id: &str,
    ) -> Result<()> {
        self.assistant = ASSISTANT_POOL.get().unwrap().assistant(id)?;
        self.state.context.assistant_id = id.to_string();
        self.save().await
    }
}

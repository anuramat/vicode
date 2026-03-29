use std::ops::ControlFlow;

use anyhow::Result;
use futures::future::try_join_all;
use tracing::debug;
use tracing::error;
use tracing::instrument;

use crate::agent::Agent;
use crate::agent::AgentHandle;
use crate::agent::AgentState;
use crate::agent::id::AgentId;
use crate::agent::replica;
use crate::agent::task::manager::TaskId;
use crate::llm::history;
use crate::llm::history::History;
use crate::llm::history::HistoryEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::message::AssistantItem;
use crate::llm::message::now_ms;
use crate::llm::provider::assistant::ASSISTANT_POOL;

#[derive(Debug)]
pub enum AgentEvent {
    TaskDone(TaskId, Result<()>),
    TaskEvent(TaskId, HistoryGeneration, HistoryEvent),
    External(ExternalEvent),
}

#[derive(Debug)]
pub enum ParentEvent {
    Started(AgentStarted),
    InfoUpdate,
    HistoryReset(History),
    HistoryUpdate(HistoryGeneration, HistoryEvent),
    TurnComplete,
    Error(String),
}

#[derive(Debug)]
pub enum ExternalEvent {
    Delete,
    Retry,
    Abort,
    Undo(usize), // TODO maybe this should send generation or whatever
    SetAssistant(String),
    Submit(UserPrompt),
    DuplicateRequest(AgentId),
}

#[derive(Debug, Clone)]
pub struct AgentStarted {
    pub aid: AgentId,
    pub state: AgentState,
    pub handle: AgentHandle,
}

#[async_trait::async_trait]
pub trait ParentSink: Send + Sync {
    async fn send(
        &self,
        event: ParentEvent,
    ) -> Result<()>;

    fn sibling(
        &self,
        aid: AgentId,
    ) -> ParentHandle;
}

pub type ParentHandle = Box<dyn ParentSink>;

#[derive(Debug)]
pub struct UserPrompt {
    pub text: Option<String>,
    pub multiplier: usize,
    pub generation: HistoryGeneration,
}

impl Agent {
    #[instrument(skip(self))]
    pub async fn handle(
        &mut self,
        event: AgentEvent,
    ) -> Result<ControlFlow<()>> {
        use AgentEvent::*;

        debug!(event = ?event, "handling agent event");

        match event {
            TaskDone(tid, result) => {
                self.handle_task_result(tid, result).await?;
            }
            TaskEvent(tid, generation, event) => {
                if self.tskmgr.pending(&tid) {
                    self.handle_history(generation, event).await?;
                }
            }
            External(event) => {
                return self.handle_external(event).await;
            }
        }
        Ok(ControlFlow::Continue(()))
    }

    pub fn idle_and(&mut self) -> Result<&mut Self> {
        anyhow::ensure!(self.tskmgr.idle(), "agent is busy");
        Ok(self)
    }

    pub fn incremented(&mut self) -> Result<HistoryGeneration> {
        let history = &mut self.idle_and()?.state.context.history;
        history.increment();
        Ok(history.generation())
    }

    async fn handle_external(
        &mut self,
        event: ExternalEvent,
    ) -> Result<ControlFlow<()>> {
        use ExternalEvent::*;
        match event {
            Undo(n) => {
                let g = self.incremented()?;
                self.handle_history(g, HistoryEvent::Pop(n)).await?;
            }
            Abort => {
                let g = self.incremented()?;
                self.handle_history(g, HistoryEvent::ResponseAborted)
                    .await?;
            }
            Delete => {
                self.tskmgr.abort().await;
                self.delete_agent().await?; // TODO maybe some special handling for failed deletes
                return Ok(ControlFlow::Break(()));
            }
            DuplicateRequest(aid) => {
                self.try_duplicate(aid).await?;
            }
            Submit(UserPrompt {
                text,
                multiplier,
                generation,
            }) => {
                if let Some(text) = text {
                    self.handle_history(generation, history::HistoryEvent::UserMessage(text))
                        .await?;
                }
                if self.tskmgr.idle() {
                    if multiplier <= 1 {
                        self.start_turn();
                    } else {
                        let replicas =
                            try_join_all((0..multiplier).map(|_| AgentId::new())).await?;
                        self.state.topology.children.extend(replicas.clone());
                        self.save().await?;
                        self.start_replica_turns(replicas);
                    }
                }
            }
            Retry => {
                if !self.tskmgr.idle() {
                    return Ok(ControlFlow::Continue(()));
                }
                self.start_turn();
            }
            SetAssistant(id) => {
                self.set_assistant(&id).await?;
            }
        }
        Ok(ControlFlow::Continue(()))
    }

    async fn handle_history(
        &mut self,
        generation: HistoryGeneration,
        event: HistoryEvent,
    ) -> Result<()> {
        // TODO verify
        self.parent
            .send(ParentEvent::HistoryUpdate(generation, event.clone()))
            .await?;
        self.state
            .context
            .history
            .handle(generation, event.clone())?;
        match event {
            HistoryEvent::ResponseStarted(_) => {}
            HistoryEvent::ResponseItem(ref item) => {
                if let AssistantItem::ToolCall(mut call) = (**item).clone()
                    && call.task.output().is_none()
                {
                    let generation = self.state.context.history.generation();
                    call.task.prepare(self)?;
                    self.tskmgr
                        .spawn(self.tx.clone(), generation, move |task| async move {
                            call.task.run().await;
                            call.executed_at_ms = Some(now_ms());
                            task.history(HistoryEvent::ResponseItem(Box::new(
                                AssistantItem::ToolCall(call),
                            )))
                            .await
                        });
                }
            }
            HistoryEvent::ResponseAborted => {
                if self.tskmgr.idle() {
                    return Ok(());
                }
                self.tskmgr.abort().await;
                // TODO wish we could move this out
                self.parent.send(ParentEvent::TurnComplete).await?;
                self.parent.send(ParentEvent::InfoUpdate).await?;
            }
            HistoryEvent::ResponseFailed(msg) => {
                error!("error in agent {}: {}", self.id, msg);
            }
            HistoryEvent::ResponseDelta(_) => return Ok(()),
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
        self.tskmgr.spawn(
            self.tx.clone(),
            context.history.generation(),
            move |task| async move {
                let result = replica::run_replicas(parent, context, replicas).await?;
                task.history(HistoryEvent::DeveloperMessage(result.report))
                    .await
            },
        );
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

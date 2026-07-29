use std::future::Future;
use std::panic::AssertUnwindSafe;

use anyhow::Result;
use futures::FutureExt;
use futures::future::AbortRegistration;
use futures::future::Abortable;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;

use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::ActivityStatus;
use crate::agent::core::AgentCore;
use crate::agent::handle::AgentEvent;
use crate::agent::router::AgentRouterHandle;
use crate::agent::router::RuntimeHandle;
use crate::llm::history::History;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::DeveloperMessage;
use crate::project::Project;

const CHANNEL_CAPACITY: usize = 100;
/// the one-line devmsg a duplicate starts with (§2.5)
pub const DUPLICATED_NOTE: &str = "this tab was duplicated from another agent; \
    the original's subagents belong to the original and are unreachable from here";

impl Agent {
    pub fn new(
        project: Project,
        router: AgentRouterHandle,
        id: AgentId,
        state: AgentState,
    ) -> Self {
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        Self::with_mailbox(project, router, id, state, tx, rx)
    }

    /// attach to a router-minted mailbox; a spawned child's seed prompt is
    /// already parked there before the runtime exists
    pub fn with_mailbox(
        project: Project,
        router: AgentRouterHandle,
        id: AgentId,
        state: AgentState,
        tx: Sender<AgentEvent>,
        rx: Receiver<AgentEvent>,
    ) -> Self {
        let (user_tx, user_rx) = channel(CHANNEL_CAPACITY);
        let (out_tx, out_rx) = channel(CHANNEL_CAPACITY);
        Self {
            core: AgentCore::new(state, project.assistants().clone()),
            project,
            id,
            router,
            rx,
            user_tx,
            user_rx,
            processed: 0,
            executor: Default::default(),
            tx,
            out_tx,
            out_rx,
            accumulators: Default::default(),
            dup_ack: None,
        }
    }

    /// Prepare handles without polling the runtime future. The caller must
    /// attach the handle through the router before launching the task.
    pub fn prepare(self) -> (RuntimeHandle, RuntimeTask) {
        let (abort, reg) = futures::future::AbortHandle::new_pair();
        let tx = self.tx.clone();
        let user_tx = self.user_tx.clone();
        (
            RuntimeHandle::new(tx, user_tx, abort),
            RuntimeTask {
                agent: self,
                registration: reg,
            },
        )
    }

    pub async fn save(&self) -> Result<()> {
        self.core.state.save(&self.project, &self.id).await
    }

    /// clone agent to given id on manual request from UI; the copy is a new
    /// root — a new, empty tab that inherits the conversation but not the
    /// original's subagents (§2.5)
    pub async fn try_duplicate(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        self.project
            .duplicate_agent_workdir(&self.id, &aid, &self.core.state.context.commit)
            .await?;
        let mut state = self.core.state.clone();
        // new empty root (§2.5): buffered messages address the original's
        // subagents, unreachable from the copy (M5)
        state.pending_messages.clear();
        let generation = state.context.history.generation();
        state.context.history.handle(
            generation,
            HistoryUpdate::DeveloperMessage(DeveloperMessage::misc(DUPLICATED_NOTE.into())),
        )?;
        let agent = Self::new(self.project.clone(), self.router.clone(), aid, state);
        agent.launch_root().await
    }

    /// register + persist + attach + launch a fresh root agent; the graph
    /// record is enqueued before the state so "durable state ⇒ durable graph
    /// record" holds on the primary paths too (L1); a failure after
    /// registration rolls the provisional node back instead of leaving it
    /// stuck `Spawning`
    pub async fn launch_root(self) -> Result<()> {
        let router = self.router.clone();
        let aid = self.id.clone();
        router.register_root(aid.clone()).await?;
        let result = async {
            // before the state, so "durable state ⇒ durable pin" holds: a pin
            // whose state never lands is swept by `cleanup::scan`, while state
            // whose base was never pinned would diff against a commit
            // nothing keeps alive. Covers both root paths — a fresh primary
            // and a duplicated tab, which inherits its `base` by clone
            self.project.pin_base(&aid, &self.core.state.context.base)?;
            self.save().await?;
            let (runtime, task) = self.prepare();
            router.attach_runtime(aid.clone(), runtime).await?;
            task.launch();
            Ok(())
        }
        .await;
        if result.is_err() {
            drop(router.rollback_spawn(aid).await);
        }
        result
    }
}

pub struct RuntimeTask {
    agent: Agent,
    registration: AbortRegistration,
}

impl RuntimeTask {
    pub fn launch(self) {
        let aid = self.agent.id.clone();
        let router = self.agent.router.clone();
        tokio::spawn(supervise(aid, router, self.agent.run(), self.registration));
    }
}

async fn supervise(
    aid: AgentId,
    router: AgentRouterHandle,
    future: impl Future<Output = Result<()>>,
    registration: AbortRegistration,
) {
    let outcome = AssertUnwindSafe(Abortable::new(future, registration))
        .catch_unwind()
        .await;
    let error = match outcome {
        Ok(Ok(Ok(()))) => "agent runtime exited unexpectedly".into(),
        Ok(Ok(Err(error))) => format!("agent runtime failed: {error:#}"),
        Ok(Err(_)) => "agent runtime cancelled unexpectedly".into(),
        Err(payload) => format!(
            "agent runtime panicked: {}",
            crate::agent::run::panic_message(&*payload)
        ),
    };
    drop(router.runtime_down(aid, error).await);
}

impl AgentState {
    /// init a primary agent from scratch
    pub fn new(
        assistant: String,
        commit: String,
        instructions: String,
    ) -> Self {
        Self {
            status: ActivityStatus::default(),
            assistant,
            context: AgentContext {
                base: commit.clone(),
                commit,
                history: History::new(instructions),
            },
            pending_messages: Vec::new(),
        }
    }

    pub async fn save(
        &self,
        project: &Project,
        id: &AgentId,
    ) -> Result<()> {
        project.store().save_state(id, self).await
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use similar_asserts::assert_eq;
    use tokio::sync::mpsc::channel;
    use tokio::time::Duration;
    use tokio::time::timeout;

    use super::*;
    use crate::agent::router::AgentRouter;
    use crate::agent::router::api::RouterError;
    use crate::agent::router::api::TurnOutcome;
    use crate::agent::router::api::WaitResult;
    use crate::agent::router::graph::NodeStatus;

    async fn supervised(
        future: impl Future<Output = Result<()>> + Send + 'static,
        cancel: bool,
    ) -> Result<WaitResult, RouterError> {
        let project = Project::new_test().unwrap().0;
        let (app_tx, _app_rx) = channel(8);
        let router = AgentRouter::spawn(
            app_tx,
            project,
            Default::default(),
            Default::default(),
            Default::default(),
        );
        let aid = AgentId::from(format!("supervised-{}", uuid::Uuid::new_v4()));
        router.register_root(aid.clone()).await.unwrap();
        let (tx, _rx) = channel(1);
        let (user_tx, _user_rx) = channel(1);
        let (abort, registration) = futures::future::AbortHandle::new_pair();
        router
            .attach_runtime(aid.clone(), RuntimeHandle::new(tx, user_tx, abort.clone()))
            .await
            .unwrap();
        tokio::spawn(supervise(aid.clone(), router.clone(), future, registration));
        if cancel {
            abort.abort();
        }
        timeout(Duration::from_secs(1), router.wait_idle(aid))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn supervisor_reports_return_error_panic_and_cancellation() {
        assert_eq!(
            supervised(async { Ok(()) }, false).await,
            Ok(WaitResult {
                status: NodeStatus::Dead,
                outcome: TurnOutcome {
                    output: None,
                    error: Some("agent runtime exited unexpectedly".into()),
                },
            })
        );
        assert_eq!(
            supervised(async { Err(anyhow::anyhow!("fatal")) }, false).await,
            Ok(WaitResult {
                status: NodeStatus::Dead,
                outcome: TurnOutcome {
                    output: None,
                    error: Some("agent runtime failed: fatal".into()),
                },
            })
        );
        assert_eq!(
            supervised(
                async {
                    panic!("boom");
                    #[allow(unreachable_code)]
                    Ok(())
                },
                false,
            )
            .await,
            Ok(WaitResult {
                status: NodeStatus::Dead,
                outcome: TurnOutcome {
                    output: None,
                    error: Some("agent runtime panicked: boom".into()),
                },
            })
        );
        assert_eq!(
            supervised(futures::future::pending(), true).await,
            Ok(WaitResult {
                status: NodeStatus::Dead,
                outcome: TurnOutcome {
                    output: None,
                    error: Some("agent runtime cancelled unexpectedly".into()),
                },
            })
        );
    }

    #[tokio::test]
    async fn try_duplicate_registers_copy_with_router() {
        let project = Project::new_test().unwrap().0;
        let (app_tx, _app_rx) = channel(8);
        let router = AgentRouter::spawn(
            app_tx,
            project.clone(),
            Default::default(),
            Default::default(),
            Default::default(),
        );

        let parent_aid = AgentId::from(format!("dup-parent-{}", uuid::Uuid::new_v4()));
        let parent_workdir = project.agent_workdir(&parent_aid);
        tokio::fs::create_dir_all(&parent_workdir).await.unwrap();
        let repo = git2::Repository::open(project.root()).unwrap();

        let mut state = project.fake_state();
        // buffered mail addresses the original's subagents: the copy must
        // not inherit it (M5)
        state
            .pending_messages
            .push(crate::llm::history::message::UserMessage::new(
                "[from: kid]\nstranded".into(),
                1,
            ));
        let mut parent = Agent::new(project.clone(), router.clone(), parent_aid.clone(), state);

        let copy_aid = router.allocate_agent_id().await.unwrap();
        let (ack, ack_rx) = tokio::sync::oneshot::channel();
        parent
            .handle(crate::agent::handle::AgentEvent::External(
                crate::agent::handle::ExternalEvent::DuplicateRequest {
                    copy: copy_aid.clone(),
                    ack,
                },
            ))
            .await
            .unwrap();
        // the ack fires only after the copy is registered (M5)
        ack_rx.await.unwrap();

        // the copy starts with the one-line "new empty tab" devmsg (§2.5)
        // and an empty buffer (new-empty-root rule, M5)
        let copy_state = project.store().load_state(&copy_aid).await.unwrap();
        assert!(copy_state.pending_messages.is_empty());
        let last = copy_state
            .context
            .history
            .state()
            .messages
            .last()
            .unwrap()
            .clone();
        assert!(
            matches!(
                &last,
                crate::llm::history::message::Message::Developer(DeveloperMessage::Misc(_))
            ) && format!("{last:?}").contains(DUPLICATED_NOTE),
            "expected the duplicated-note devmsg, got {last:?}"
        );

        // the copy pins its inherited base under its *own* name: it clones
        // `base` from the original, so without a pin of its own it would be
        // left diffing against a commit nothing keeps alive once the
        // original — and with it `refs/vicode/base/<original>` — is deleted
        similar_asserts::assert_eq!(
            repo.find_reference(&project.base_ref(&copy_aid))
                .unwrap()
                .target()
                .unwrap()
                .to_string(),
            copy_state.context.base
        );

        // observable via router: shutdown succeeds only if registered
        router.shutdown(copy_aid).await.unwrap();
    }
}

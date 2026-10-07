use anyhow::Result;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::mpsc::unbounded_channel;

use crate::agent::Agent;
use crate::agent::AgentContext;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::handle::LOST_ON_RESTART;
use crate::agent::router::Router;
use crate::agent::task::executor::TaskExecutor;
use crate::agent::task::ledger::TaskLedger;
use crate::llm::history::History;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::DeveloperMessage;
use crate::project::Project;
use crate::tui::app::AppEvent;

pub const DUPLICATED_NOTE: &str = "this tab was duplicated from another agent; the original's subagents belong to the original and are unreachable from here";

impl Agent {
    pub fn new(
        project: Project,
        router: Router,
        app_tx: UnboundedSender<AppEvent>,
        id: AgentId,
        mut state: AgentState,
    ) -> Self {
        // restore repair: a dangling function_call in history would 400 every later turn
        state
            .context
            .history
            .fail_unresolved_tool_calls(LOST_ON_RESTART);
        let (user_tx, user_rx) = unbounded_channel();
        let (task_tx, task_rx) = unbounded_channel();
        Self {
            project,
            id,
            state,
            ledger: TaskLedger::default(),
            compaction: None,
            needs_turn: false,
            dirty: false,
            router,
            app_tx,
            user_tx,
            user_rx,
            executor: TaskExecutor::default(),
            task_tx,
            task_rx,
        }
    }

    pub async fn save(&self) -> Result<()> {
        self.state.save(&self.project, &self.id).await
    }

    /// clone an idle agent into a new root under `aid`
    pub async fn try_duplicate(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        self.idle()?;
        self.project
            .duplicate_agent_workdir(&self.id, &aid, &self.state.context.commit)
            .await?;
        let mut state = self.state.clone();
        state.pending_messages.clear();
        let generation = state.context.history.generation();
        state.context.history.handle(
            generation,
            HistoryUpdate::DeveloperMessage(DeveloperMessage::misc(DUPLICATED_NOTE.into())),
        )?;
        let agent = Self::new(
            self.project.clone(),
            self.router.clone(),
            self.app_tx.clone(),
            aid,
            state,
        );
        agent.launch_root().await
    }

    /// persist + start a fresh root agent
    pub async fn launch_root(self) -> Result<()> {
        let router = self.router.clone();
        let aid = self.id.clone();
        let result = async {
            self.save().await?;
            router.start(self, None).await
        }
        .await;
        if result.is_err() {
            drop(router.rollback_spawn(&aid).await);
        }
        result
    }
}

impl AgentState {
    /// init a primary agent from scratch
    pub fn new(
        assistant_id: String,
        commit: String,
        instructions: String,
    ) -> Self {
        Self {
            assistant_id,
            context: AgentContext {
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
    use super::*;

    #[tokio::test]
    async fn try_duplicate_registers_copy_with_router() {
        let project = Project::new_test().unwrap().0;
        let (app_tx, mut app_rx) = unbounded_channel();
        let router = Router::new(app_tx.clone(), project.clone());

        let parent_aid = AgentId::from(format!("dup-parent-{}", uuid::Uuid::new_v4()));
        let parent_workdir = project.agent_workdir(&parent_aid);
        tokio::fs::create_dir_all(&parent_workdir).await.unwrap();

        let mut state = project.fake_state();
        // buffered mail addresses the original's subagents: the copy must
        // not inherit it
        state
            .pending_messages
            .push(crate::llm::history::message::PeerMessage::new("kid", "stranded", 1).into());
        let mut parent = Agent::new(
            project.clone(),
            router.clone(),
            app_tx,
            parent_aid.clone(),
            state,
        );

        let copy_aid = router.allocate_agent_id();
        parent
            .handle(
                0,
                crate::agent::event::AgentEvent::User(crate::agent::event::UserCommand::Duplicate(
                    copy_aid.clone(),
                )),
            )
            .await
            .unwrap();
        // the copy registered in-handler: nothing reported a failure
        while let Ok(event) = app_rx.try_recv() {
            assert!(
                !matches!(
                    event,
                    crate::tui::app::AppEvent::Agent(
                        _,
                        crate::agent::event::UiEvent::DuplicateFailed { .. }
                    )
                ),
                "{event:?}"
            );
        }

        // the copy starts with the one-line "new empty tab" devmsg
        // and an empty buffer (new-empty-root rule)
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

        // observable via router: shutdown succeeds only if registered
        router.shutdown(&copy_aid).unwrap();
    }
}

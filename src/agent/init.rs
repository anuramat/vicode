use anyhow::Result;
use tokio::sync::mpsc::unbounded_channel;

use crate::agent::Agent;
use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::handle::LOST_ON_RESTART;
use crate::agent::router::Router;
use crate::agent::task::Tasks;
use crate::llm::history::History;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::DeveloperMessage;
use crate::project::Project;

pub const DUPLICATED_NOTE: &str = "this tab was duplicated from another agent; the original's subagents belong to the original and are unreachable from here";

impl Agent {
    pub fn new(
        router: Router,
        id: AgentId,
        mut state: AgentState,
    ) -> Self {
        // restore repair: a dangling function_call in history would 400 every later turn
        state.history.fail_unresolved_tool_calls(LOST_ON_RESTART);
        let (tx, rx) = unbounded_channel();
        Self {
            project: router.project.clone(),
            id,
            state,
            tasks: Tasks::new(tx.clone()),
            compaction: None,
            needs_turn: false,
            dirty: false,
            app_tx: router.app_tx.clone(),
            router,
            tx,
            rx,
        }
    }

    /// clone an idle agent into a new root under `aid`
    pub async fn try_duplicate(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        self.ensure_idle()?;
        let mut state = self.state.clone();
        let generation = state.history.generation();
        state.history.handle(
            generation,
            HistoryUpdate::DeveloperMessage(DeveloperMessage::misc(DUPLICATED_NOTE.into())),
        )?;
        let project = self.project.clone();
        let (original, copy) = (self.id.clone(), aid.clone());
        let setup = async move {
            project
                .duplicate_agent_workdir(&original, &copy, &state.commit)
                .await?;
            Ok(state)
        };
        self.router.create(aid, None, setup).await?;
        Ok(())
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
            commit,
            history: History::new(instructions),
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
        let router = Router::new(app_tx, project.clone());

        let parent_aid = AgentId::from(format!("dup-parent-{}", uuid::Uuid::new_v4()));
        let parent_workdir = project.agent_workdir(&parent_aid);
        tokio::fs::create_dir_all(&parent_workdir).await.unwrap();

        let mut state = project.fake_state();
        // buffered mail is undelivered history: the copy inherits it, like
        // the earlier mail from the original's subagents
        state
            .pending_messages
            .push(crate::llm::history::message::PeerMessage::new("kid", "stranded", 1).into());
        let mut parent = Agent::new(router.clone(), parent_aid.clone(), state);

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
        // the copy registered in-handler: nothing reported a failure before
        // its Started, which carries the state before resume flushes the buffer
        let copy_state = loop {
            match app_rx.recv().await.unwrap() {
                crate::tui::app::AppEvent::Agent(
                    aid,
                    crate::agent::event::UiEvent::Started { state, .. },
                ) if aid == copy_aid => break state,
                event => assert!(
                    !matches!(
                        event,
                        crate::tui::app::AppEvent::Agent(
                            _,
                            crate::agent::event::UiEvent::DuplicateFailed { .. }
                        )
                    ),
                    "{event:?}"
                ),
            }
        };

        // the copy starts with the original's buffer and the one-line
        // duplicated-note devmsg
        insta::assert_yaml_snapshot!(copy_state.pending_messages, @r#"
        - Peer:
            text: "[from: kid]\nstranded"
            token_count: 6
            created_at: 1
        "#);
        let last = copy_state.history.state().messages.last().unwrap().clone();
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

use anyhow::Result;

use crate::agent::AgentStatus;
use crate::llm::history::Entries;
use crate::llm::history::History;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::message::Message;
use crate::llm::message::UserMessage;
use crate::project::Project;
use crate::project::layout::LayoutTrait;
use crate::tui::app::AppEvent;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::Tab;

impl Tab<'_> {
    pub fn set_osc7(
        &self,
        project: &Project,
    ) {
        let path = project.agent_workdir(&self.aid);
        set_osc7(&path);
    }

    pub async fn update(
        &mut self,
        generation: HistoryGeneration,
        event: HistoryUpdate,
    ) -> Result<()> {
        let input = if let HistoryUpdate::Pop(n) = event {
            Some(self.combined_user_msgs(n))
        } else {
            None
        };
        self.agent
            .state
            .context
            .history
            .handle(generation, event)
            .expect("history desync");
        // XXX proper handling -- resync and show error notification
        if let Some(input) = input {
            self.user_input.0.prepend_text(input);
            self.update_input_border();
        }
        // NOTE for now we only change the last element, or drop/add stuff. if in the future we edit messages in the middle, we will need to change this logic
        self.scroll
            .set_dirty(self.agent.state.context.history.len().saturating_sub(1));
        self.scroll.set_len(self.agent.state.context.history.len());
        Ok(())
    }

    pub fn replace_history(
        &mut self,
        history: History,
    ) {
        self.agent.state.context.history = history;
        self.scroll = Default::default();
    }

    // XXX does this make sense
    pub async fn set_state(
        &mut self,
        status: AgentStatus,
    ) -> Result<bool> {
        if self.agent.state.status == status {
            return Ok(false);
        }
        self.agent.state.status = status;
        self.tx
            .send(AppEvent::TabStatusChanged(self.aid.clone()))
            .await?;
        Ok(true)
    }

    pub fn combined_user_msgs(
        &self,
        popped: usize,
    ) -> String {
        // NOTE we only apply the results if history event was successfully handled, so we don't have to check it here
        let mut result = Vec::new();
        let entries: &Entries = &self.agent.state.context.history;
        for entry in &entries[entries.len().saturating_sub(popped)..] {
            if let Message::User(UserMessage { ref text }) = entry.message {
                result.push(text.clone());
            }
        }
        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use futures::future::AbortHandle;
    use similar_asserts::assert_eq;
    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::agent::AgentContext;
    use crate::agent::AgentHandle;
    use crate::agent::AgentState;
    use crate::agent::AgentStatus;
    use crate::agent::AgentTopology;
    use crate::agent::handle::AgentEvent;
    use crate::agent::id::AgentId;
    use crate::config::Config;
    use crate::llm::provider::assistant::Assistant;
    use crate::llm::provider::assistant::AssistantPool;

    async fn assistant() -> Assistant {
        AssistantPool::from_config(
            &Config::parse_with_defaults(
                r#"
                primary_assistant = ["test"]
                shell_cmd = ["bash", "-c"]

                [sandbox]
                kind = "bwrap"
                bin = "bwrap"
                args = []
                stages = []

                [keymap.cmdline]

                [keymap.normal]

                [keymap.insert]

                [providers.main]
                base_url = "https://api.example.com/v1"

                [assistants.test]
                provider = "main"
                model = "gpt-test"
                "#,
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .assistant("test")
        .unwrap()
    }

    #[tokio::test]
    async fn set_state_updates_tab_status() {
        let (tx, mut rx) = channel(1);
        let (agent_tx, _agent_rx) = channel::<AgentEvent>(1);
        let aid = AgentId::from("tab-compact".to_string());
        let state = AgentState {
            status: AgentStatus::Idle,
            assistant: assistant().await,
            topology: AgentTopology::default(),
            context: AgentContext {
                ..Default::default()
            },
        };
        let mut tab = Tab::new(
            tx,
            aid.clone(),
            AgentHandle {
                tx: agent_tx,
                state,
                abort: AbortHandle::new_pair().0,
            },
        )
        .await
        .unwrap();

        tab.set_state(AgentStatus::Compacting).await.unwrap();

        assert_eq!(tab.agent.state.status, AgentStatus::Compacting);
        assert!(matches!(
            rx.recv().await,
            Some(AppEvent::TabStatusChanged(changed)) if changed == aid
        ));
    }
}

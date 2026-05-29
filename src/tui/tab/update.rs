use anyhow::Result;

use crate::agent::AgentStatus;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::Message;
use crate::llm::history::message::UserMessage;
use crate::project::layout::LayoutTrait;
use crate::tui::app::AppEvent;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::Tab;

impl Tab<'_> {
    pub fn set_osc7(&self) {
        let path = self.project.agent_workdir(&self.aid);
        set_osc7(&path);
    }

    pub fn update(
        &mut self,
        generation: HistoryGeneration,
        event: HistoryUpdate,
    ) -> Result<()> {
        let input = if let HistoryUpdate::Pop(n) = event {
            Some(self.combined_user_msgs(n))
        } else {
            None
        };
        self.history_mut().handle(generation, event)?;

        // XXX proper handling -- resync and show error notification
        if let Some(input) = input {
            self.input.prepend_text(input);
            self.update_input_title();
        }
        // NOTE for now we only change the last element, or drop/add stuff. if in the future we edit messages in the middle, we will need to change this logic
        let len = self.state.context.history.state().messages.len();
        self.scroll.set_dirty(len.saturating_sub(1));
        self.scroll.set_len(len);
        Ok(())
    }

    // XXX does this make sense
    pub async fn set_state(
        &mut self,
        status: AgentStatus,
    ) -> Result<bool> {
        if self.state.status == status {
            return Ok(false);
        }
        self.state.status = status;
        self.refresh_file_completion()?;
        if let Some(router) = &self.router {
            router
                .app_tx()
                .send(AppEvent::TabStatusChanged(self.aid.clone()))
                .await?;
        }
        Ok(true)
    }

    pub fn combined_user_msgs(
        &self,
        popped: usize,
    ) -> String {
        // NOTE we only apply the results if history event was successfully handled, so we don't have to check it here
        let mut result = Vec::new();
        let messages = &self.state.context.history.state().messages;
        let start = messages.len().saturating_sub(popped);
        for msg in &messages[start..] {
            if let Message::User(UserMessage { text, .. }) = msg {
                result.push(text.clone());
            }
        }
        result.join("\n")
    }
}

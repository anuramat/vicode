use anyhow::Result;

use crate::llm::history::History;
use crate::llm::history::HistoryEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::message::HistoryEntry;
use crate::llm::message::Message;
use crate::llm::message::UserMessage;
use crate::project::PROJECT;
use crate::tui::app::AppEvent;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::AssistantState;
use crate::tui::tab::Tab;
use crate::tui::tab::TabState;

impl Tab<'_> {
    pub fn set_osc7(&self) {
        let path = PROJECT.agent_workdir(&self.aid);
        set_osc7(&path);
    }

    pub fn update(
        &mut self,
        generation: HistoryGeneration,
        event: HistoryEvent,
    ) {
        let input = if let HistoryEvent::Pop(n) = event {
            Some(self.combined_user_msgs(n))
        } else {
            None
        };
        let delta = self
            .agent_state
            .context
            .history
            .handle(generation, event)
            .expect("history desync");
        // XXX proper handling -- resync and show error notification
        if let Some(input) = input {
            self.user_input.0.prepend_text(input);
            self.update_input_border();
        }
        self.context_tokens = self.context_tokens.saturating_add_signed(delta);
        // NOTE for now we only change the last element, or drop/add stuff. if in the future we edit messages in the middle, we will need to change this logic
        self.scroll
            .set_dirty(self.agent_state.context.history.len().saturating_sub(1));
        self.scroll.set_len(self.agent_state.context.history.len());
    }

    pub fn replace_history(
        &mut self,
        history: History,
    ) {
        self.context_tokens = history.total_tokens();
        self.agent_state.context.history = history;
        self.scroll = Default::default();
    }

    // XXX does this make sense
    pub async fn set_state(
        &mut self,
        state: TabState,
    ) -> Result<()> {
        if self.state == state {
            return Ok(());
        }
        self.state = state;
        self.tx
            .send(AppEvent::TabStatusChanged(self.aid.clone()))
            .await?;
        Ok(())
    }

    pub async fn sync_state_from_history(&mut self) -> Result<()> {
        self.set_state(TabState::Running(AssistantState::from_history(
            self.agent_state.context.history.as_ref(),
        )))
        .await
    }

    pub fn combined_user_msgs(
        &self,
        popped: usize,
    ) -> String {
        // NOTE we only apply the results if history event was successfully handled, so we don't have to check it here
        let mut result = Vec::new();
        let entries = self.agent_state.context.history.as_ref();
        for entry in &entries[entries.len().saturating_sub(popped)..] {
            if let Message::User(UserMessage { ref text }) = entry.message {
                result.push(text.clone());
            }
        }
        result.join("\n")
    }
}

use anyhow::Result;

use crate::llm::history::HistoryUpdate;
use crate::llm::history::Splice;
use crate::llm::history::message::Message;
use crate::llm::history::message::UserMessage;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::Tab;

impl Tab<'_> {
    pub fn set_osc7(&self) {
        let path = self.project.agent_workdir(&self.aid);
        set_osc7(&path);
    }

    pub fn update(
        &mut self,
        event: HistoryUpdate,
    ) -> Result<()> {
        let input = if let HistoryUpdate::Pop(n) = &event {
            Some(self.combined_user_msgs(*n))
        } else {
            None
        };
        let splice = self.history_mut().handle(event)?;
        if let Some(input) = input {
            self.input.prepend_text(input);
            self.update_input_title();
        }
        self.mirror(splice.as_ref())
    }

    /// apply the history's splice to the scroll cache; a splice that doesn't
    /// fit it is a bug, so the cache is rebuilt instead of rendering a
    /// corrupt view, and the bug is reported
    fn mirror(
        &mut self,
        splice: Option<&Splice>,
    ) -> Result<()> {
        if splice.is_some_and(|s| s.range.end > self.scroll.len()) {
            return self.resync(splice);
        }
        if let Some(Splice { range, len }) = splice.cloned() {
            self.scroll.splice(range, len);
        }
        if self.scroll.len() != self.state.history.state().messages.len() {
            return self.resync(splice);
        }
        Ok(())
    }

    fn resync(
        &mut self,
        splice: Option<&Splice>,
    ) -> Result<()> {
        self.reset_scroll();
        anyhow::bail!("history view went out of sync and was rebuilt; splice: {splice:?}")
    }

    pub fn combined_user_msgs(
        &self,
        popped: usize,
    ) -> String {
        // NOTE we only apply the results if history event was successfully handled, so we don't have to check it here
        let mut result = Vec::new();
        let messages = &self.state.history.state().messages;
        let start = messages.len().saturating_sub(popped);
        for msg in &messages[start..] {
            if let Message::User(UserMessage { text, .. }) = msg {
                result.push(text.clone());
            }
        }
        result.join("\n")
    }
}

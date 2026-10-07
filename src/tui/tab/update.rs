use anyhow::Result;

use crate::llm::history::AssistantEvent;
use crate::llm::history::HistoryGeneration;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::message::AssistantItem;
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
        generation: HistoryGeneration,
        event: HistoryUpdate,
    ) -> Result<()> {
        let input = if let HistoryUpdate::Pop(n) = &event {
            Some(self.combined_user_msgs(*n))
        } else {
            None
        };
        if let Some(call_id) = finalized_call(&event) {
            self.live_output.remove(call_id);
        }
        // a compaction rewrites the front of the history too
        if matches!(event, HistoryUpdate::Compact(_)) {
            self.scroll.set_len(0);
        }
        self.history_mut().handle(generation, event)?;
        if let Some(input) = input {
            self.input.prepend_text(input);
            self.update_input_title();
        }
        // NOTE for now we only change the last element, or drop/add stuff. if in the future we edit messages in the middle, we will need to change this logic
        let len = self.state.history.state().messages.len();
        self.scroll.set_dirty(len.saturating_sub(1));
        self.scroll.set_len(len);
        Ok(())
    }

    /// tee'd live output of an in-flight call; authoritative text arrives
    /// with the finalized item, so this is render state only
    pub fn stream_tool_output(
        &mut self,
        call_id: String,
        chunk: &str,
    ) {
        self.live_output.entry(call_id).or_default().push_str(chunk);
        // the pending call lives in the last message: nothing appends while
        // the agent is busy
        let len = self.state.history.state().messages.len();
        self.scroll.set_dirty(len.saturating_sub(1));
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

/// the call an update gives its real output; its live buffer is done
fn finalized_call(event: &HistoryUpdate) -> Option<&str> {
    match event {
        HistoryUpdate::TurnResponse(AssistantEvent::Item(item)) => match &**item {
            AssistantItem::ToolCall(call) if call.task.output().is_some() => Some(&call.call_id),
            _ => None,
        },
        HistoryUpdate::ToolCallFailed { call_id, .. } => Some(call_id),
        _ => None,
    }
}

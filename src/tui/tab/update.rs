use crate::llm::history::HistoryEvent;
use crate::project::PROJECT;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::Tab;

impl Tab<'_> {
    pub fn set_osc7(&self) {
        let path = PROJECT.agent_workdir(&self.aid);
        set_osc7(&path);
    }

    pub fn update(
        &mut self,
        event: HistoryEvent,
    ) {
        self.agent_state.context.history.handle(event.clone());
        match event {
            HistoryEvent::ResponseDelta(idx, _)
            | HistoryEvent::ResponseItem(idx, _)
            | HistoryEvent::ResponseCompleted(idx, _)
            | HistoryEvent::ResponseFailed(idx, _)
            | HistoryEvent::UserMessage(idx, _)
            | HistoryEvent::DeveloperMessage(idx, _) => {
                self.scroll.set_dirty(idx);
            }
        }
    }
}

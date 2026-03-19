use crate::llm::history::HistoryEvent;
use crate::llm::history::HistoryLoc;
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
        loc: HistoryLoc,
        event: HistoryEvent,
    ) {
        let delta = self.agent_state.context.history.handle(loc, event.clone());
        self.context_tokens = self.context_tokens.saturating_add_signed(delta);
        self.scroll.set_dirty(loc);
    }
}

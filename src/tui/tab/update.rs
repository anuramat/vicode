use anyhow::Result;

use crate::llm::history::History;
use crate::llm::history::HistoryEvent;
use crate::llm::history::HistoryLoc;
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
        loc: HistoryLoc,
        event: HistoryEvent,
    ) {
        let token_count_delta = self.agent_state.context.history.handle(loc, event);
        self.context_tokens = self.context_tokens.saturating_add_signed(token_count_delta);
        self.scroll.set_dirty(loc);
    }

    pub fn replace_history(
        &mut self,
        history: History,
    ) {
        self.context_tokens = history.total_tokens();
        self.agent_state.context.history = history;
        self.scroll = Default::default();
    }

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
}

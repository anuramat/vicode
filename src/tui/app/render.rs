use anyhow::Result;
use ratatui::prelude::*;

use crate::tui::app::App;
use crate::tui::widgets::logo::LOGO_VARIANTS;
use crate::tui::widgets::statusline::StatusLine;

const TAB_PANE_WIDTH: u16 = 24;

const CONSTRAINTS: [Constraint; 2] = [Constraint::Length(TAB_PANE_WIDTH), Constraint::Min(0)];

// TODO skip tablist and info pane if terminal is too small

impl<'a> App<'a> {
    #[tracing::instrument(skip(self))]
    pub fn draw(&mut self) -> Result<()> {
        let selected = self.selected_tab();
        self.terminal.draw(|frame| {
            // statusline vs the rest
            let outer = Layout::default()
                .direction(Direction::Vertical)
                .constraints(vec![Constraint::Min(0), Constraint::Length(1)])
                .split(frame.area());

            // tablist vs tab content
            let inner = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(CONSTRAINTS)
                .split(outer[0]);
            frame.render_stateful_widget(&self.tablist.widget, inner[0], &mut self.tablist.state);

            let mut tab_name = None;
            if let Some(tabnum) = selected
                && let Some((_, tab)) = self.tabs.get_index_mut(tabnum)
            {
                tab.render(inner[1], frame.buffer_mut(), self.ctx);
                tab_name = Some(tab.aid.0.to_string());
            } else {
                frame.render_widget(&*LOGO_VARIANTS, frame.area());
            }
            let line = StatusLine::new(self.project_name.clone(), tab_name);
            frame.render_widget(&line, outer[1]);
        })?;
        Ok(())
    }
}

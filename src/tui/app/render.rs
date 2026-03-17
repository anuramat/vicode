use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::prelude::*;
use ratatui::text::Line;

use crate::tui::app::App;
use crate::tui::widgets::logo::LOGO_VARIANTS;

const TAB_PANE_WIDTH: u16 = 24;

const CONSTRAINTS: [Constraint; 2] = [Constraint::Length(TAB_PANE_WIDTH), Constraint::Min(0)];

// TODO skip tablist and info pane if terminal is too small

impl<'a> App<'a> {
    #[tracing::instrument(skip(self, term))]
    pub fn draw(
        &mut self,
        term: &mut DefaultTerminal,
    ) -> Result<()> {
        let selected = self.selected_tab();
        term.draw(|frame| {
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

            let mut tab_info = None;
            if let Some(tabnum) = selected
                && let Some((_, tab)) = self.tabs.get_index_mut(tabnum)
            {
                tab.render(inner[1], frame.buffer_mut(), self.ctx);
                tab_info = Some(TabInfo {
                    name: tab.aid.to_string(),
                    assistant: tab.agent_state.context.assistant_id.clone(),
                });
            } else {
                frame.render_widget(&*LOGO_VARIANTS, frame.area());
            }
            let line = self.status_line(tab_info, outer[1].width);
            frame.render_widget(&line, outer[1]);
        })?;
        Ok(())
    }

    fn status_line(
        &'a self,
        tab: Option<TabInfo>,
        width: u16,
    ) -> Line<'a> {
        if let Some(msg) = self.notification.as_ref() {
            return Line::raw(&msg.msg);
        }

        let mut line = Line::raw("");
        line.push_span(Span::styled(&self.project_name, Style::new().dark_gray()));
        let Some(tab) = tab else { return line };
        line.push_span(Span::styled("/", Style::new().dark_gray()));
        line.push_span(Span::raw(tab.name));

        let remaining: usize = (width as usize).saturating_sub(line.width());
        if tab.assistant.len() + 3 < remaining {
            let spacing: usize = remaining - tab.assistant.len();
            line.push_span(" ".repeat(spacing));
            line.push_span(tab.assistant);
        }
        line
    }
}

struct TabInfo {
    name: String,
    assistant: String,
}

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::prelude::*;
use ratatui::text::Line;
use ratatui::widgets::Widget;

use crate::tui::app::App;
use crate::tui::tab::TabState;
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
        let selected = self.selected_tab_idx();
        term.draw(|frame| {
            let [body_area, line_area] = *Layout::default()
                .direction(Direction::Vertical)
                .constraints(vec![Constraint::Min(0), Constraint::Length(1)])
                .split(frame.area())
            else {
                unreachable!();
            };

            let [tablist_area, tab_area] = *Layout::default()
                .direction(Direction::Horizontal)
                .constraints(CONSTRAINTS)
                .split(body_area)
            else {
                unreachable!();
            };
            frame.render_stateful_widget(
                &self.tablist.widget,
                tablist_area,
                &mut self.tablist.state,
            );

            let mut tab_info = None;
            if let Some(tabnum) = selected
                && let Some((_, tab)) = self.tabs.get_index_mut(tabnum)
            {
                tab.render(tab_area, frame.buffer_mut(), self.ctx);
                tab_info = Some(TabInfo {
                    name: tab.aid.to_string(),
                    state: tab.state.clone(),
                    assistant: tab.agent_state.context.assistant_id.clone(),
                    context_tokens: tab.context_tokens,
                    instruction_tokens: tab.instructions_tokens,
                });
            } else {
                frame.render_widget(&*LOGO_VARIANTS, frame.area());
            }
            if let Some(cmdline) = self.cmdline.as_mut() {
                let n_lines = cmdline.lines().len() as u16;
                let line_area = Rect {
                    y: line_area.y.saturating_sub(n_lines) + 1,
                    height: n_lines,
                    ..line_area
                };
                let [char_area, input_area] = *Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints(vec![Constraint::Length(1), Constraint::Min(0)])
                    .split(line_area)
                else {
                    unreachable!()
                };
                frame.render_widget(Line::raw(":"), char_area);
                cmdline.render(input_area, frame.buffer_mut());
            } else {
                let line = self.status_line(tab_info, line_area.width);
                frame.render_widget(&line, line_area);
            }
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

        let tokens = format!(
            "{:.1} + {:.1} kT",
            tab.instruction_tokens as f64 / 1000.0,
            tab.context_tokens as f64 / 1000.0
        );
        // TODO prettier tab status presentation using display macro from serde_plain
        let assistant = format!("{} | {:?} | {}", tokens, tab.state, tab.assistant);
        if assistant.len() + 3 < remaining {
            let spacing: usize = remaining - assistant.len();
            line.push_span(" ".repeat(spacing));
            line.push_span(assistant);
        }
        line
    }
}

struct TabInfo {
    name: String,
    assistant: String,
    context_tokens: usize,
    instruction_tokens: usize,
    state: TabState,
}

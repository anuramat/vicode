use anyhow::Result;
use ratatui::prelude::*;
use ratatui::text::Line;
use ratatui::widgets::Clear;

use crate::tui::app::App;
use crate::tui::app::NotificationKind::Error;
use crate::tui::app::NotificationKind::Info;
use crate::tui::colors::PANE_BG_COLOR;
use crate::tui::colors::STL_BG;
use crate::tui::colors::STL_DIM_FG;
use crate::tui::colors::STL_ERROR_BG;
use crate::tui::colors::STL_FG;
use crate::tui::widgets::logo::LOGO_VARIANTS;

const TABLIST_WIDTH: u16 = 24;

const CONSTRAINTS: [Constraint; 2] = [Constraint::Min(0), Constraint::Length(1)];

impl<'a> App<'a> {
    #[tracing::instrument(skip(self, term))]
    pub fn draw<B>(
        &mut self,
        term: &mut Terminal<B>,
    ) -> Result<()>
    where
        B: ratatui::backend::Backend,
    {
        tracing::debug!("start app render");
        term.draw(|frame| {
            let [body_area, line_area] = *Layout::default()
                .direction(Direction::Vertical)
                .constraints(CONSTRAINTS)
                .split(frame.area())
            else {
                unreachable!();
            };

            let tablist_width = if self.show_tabs { TABLIST_WIDTH } else { 3 };
            let [tablist_area, tab_area] = *Layout::default()
                .direction(Direction::Horizontal)
                .constraints(vec![Constraint::Length(tablist_width), Constraint::Min(0)])
                .split(body_area)
            else {
                unreachable!();
            };
            frame
                .buffer_mut()
                .set_style(tablist_area, Style::default().bg(PANE_BG_COLOR));
            frame.render_stateful_widget(
                &self.tablist.widget,
                tablist_area,
                &mut self.tablist.state,
            );

            let ctx = self.ctx;
            let layout = self.project.config().layout;
            if let Some(idx) = self.selected_tab_idx() {
                if let Some((_, tab)) = self.tabs.get_index_mut(idx) {
                    tab.render(tab_area, frame.buffer_mut(), ctx, layout);
                }
            } else {
                frame.render_widget(&*LOGO_VARIANTS, frame.area());
            }

            if self.cmdline.input.focused() {
                self.cmdline.render(line_area, frame.buffer_mut());
            } else {
                frame.render_widget(&Clear, line_area);
                let stl = self.status_line(line_area.width);
                frame.render_widget(&stl, line_area);
            }
        })?;
        tracing::debug!("end app render");
        Ok(())
    }

    fn status_line(
        &'a self,
        width: u16,
    ) -> Line<'a> {
        let mut line = Line::raw("");
        let mut bg = STL_BG;

        if let Some(msg) = self.notification.as_ref() {
            bg = match msg.kind {
                Info => STL_BG,
                Error => STL_ERROR_BG,
            };
            line.push_span(Span::styled(&msg.msg, Style::new().fg(STL_FG)));
        } else if let Ok(tab) = self.selected_tab() {
            line.push_span(Span::styled(
                format!("{}/", &self.project_name),
                Style::new().fg(STL_DIM_FG),
            ));
            line.push_span(Span::styled(tab.aid.to_string(), Style::new().fg(STL_FG)));
        } else {
            line.push_span(Span::styled(&self.project_name, Style::new().fg(STL_FG)));
        }

        if let Ok(tab) = self.selected_tab() {
            let remaining_width: usize = (width as usize).saturating_sub(line.width());

            #[allow(clippy::cast_precision_loss)]
            let tokens = {
                let window = tab
                    .agent
                    .state
                    .assistant
                    .config
                    .window
                    .map(|window| format!(" / {:.1}", window as f64 / 1000.0))
                    .unwrap_or_default();
                format!(
                    "{:.1}{} kT",
                    tab.agent.state.context.history.total_tokens() as f64 / 1000.0,
                    window
                )
            };

            let right_part = format!(
                "{} | {} | {}",
                tokens, tab.agent.state.status, tab.agent.state.assistant.id
            );
            // TODO +3 move to a const
            if right_part.len() + 3 < remaining_width {
                let spacing: usize = remaining_width - right_part.len();
                line.push_span(" ".repeat(spacing));
                line.push_span(Span::styled(right_part, Style::new().fg(STL_FG)));
            }
        }
        line.bg(bg)
    }
}

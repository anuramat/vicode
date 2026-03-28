use anyhow::Result;
use ratatui::prelude::*;
use ratatui::text::Line;

use crate::tui::app::App;
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
            frame.render_stateful_widget(
                &self.tablist.widget,
                tablist_area,
                &mut self.tablist.state,
            );

            let ctx = self.ctx;
            if let Some(idx) = self.selected_tab_idx() {
                if let Some((_, tab)) = self.tabs.get_index_mut(idx) {
                    tab.render(tab_area, frame.buffer_mut(), ctx);
                }
            } else {
                frame.render_widget(&*LOGO_VARIANTS, frame.area());
            }

            if self.cmdline.input.focus {
                self.cmdline.render(line_area, frame.buffer_mut());
            } else {
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
        if let Some(msg) = self.notification.as_ref() {
            return Line::raw(&msg.msg);
        }

        let mut line = Line::raw("");
        line.push_span(Span::styled(&self.project_name, Style::new().dark_gray()));
        let Ok(tab) = self.selected_tab() else {
            return line;
        };
        line.push_span(Span::styled("/", Style::new().dark_gray()));
        line.push_span(Span::raw(tab.aid.to_string()));

        let remaining: usize = (width as usize).saturating_sub(line.width());

        let tokens = {
            let window = if let Some(window) = tab.assistant_config.model.window {
                format!(" / {:.1}", window as f64 / 1000.0)
            } else {
                "".to_string()
            };
            format!(
                "{:.1} + {:.1}{} kT",
                tab.instructions_tokens as f64 / 1000.0,
                tab.context_tokens as f64 / 1000.0,
                window
            )
        };
        // TODO prettier status
        let right_part = format!(
            "{} | {:?} | {}",
            tokens, tab.state, tab.agent_state.context.assistant_id
        );
        if right_part.len() + 3 < remaining {
            let spacing: usize = remaining - right_part.len();
            line.push_span(" ".repeat(spacing));
            line.push_span(right_part);
        }
        line
    }
}

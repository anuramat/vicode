use ratatui::prelude::*;
use ratatui::text::Line;

use crate::tui::app::App;
use crate::tui::app::NotificationKind::Error;
use crate::tui::app::NotificationKind::Info;
use crate::tui::colors::STL_BG;
use crate::tui::colors::STL_DIM_FG;
use crate::tui::colors::STL_ERROR_BG;
use crate::tui::colors::STL_FG;

impl<'a> App<'a> {
    pub(super) fn status_line(
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
                    .state
                    .assistant
                    .config
                    .window
                    .map(|window| format!(" / {:.1}", window as f64 / 1000.0))
                    .unwrap_or_default();
                format!(
                    "{:.1}{} kT",
                    tab.history().token_count() as f64 / 1000.0,
                    window
                )
            };

            let right_part = format!(
                "{} | {} | {}",
                tokens, tab.state.status, tab.state.assistant.id
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

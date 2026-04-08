use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::style::Style;
use ratatui::widgets::Clear;
use ratatui::widgets::List;
use ratatui::widgets::StatefulWidget;
use ratatui::widgets::Widget;

use super::Input;

impl Input<'_> {
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        self.textarea.render(area, buf);

        let Some(active) = self.completion.active().as_ref() else {
            return;
        };
        let matches = self.completion.items();
        if !matches.is_empty() {
            let width = matches
                .iter()
                .map(|item| item.value().chars().count())
                .max()
                .unwrap_or(0) as u16;
            let height = (matches.len() as u16).min(self.completion.max_height());
            let (row, _) = self.textarea.cursor();
            let completion_area = Rect {
                x: area.x + active.start() as u16,
                y: area.y.saturating_sub(height) + row as u16,
                width,
                height,
            }
            .intersection(buf.area);
            // TODO make the color stand out, maybe lighter bg?
            Clear.render(completion_area, buf);
            buf.set_style(
                completion_area,
                Style::new().bg(crate::tui::colors::COMPLETION_BG_COLOR),
            );
            StatefulWidget::render(
                List::new(matches.iter().map(|item| item.rendered().clone()))
                    .highlight_style(Style::new().reversed()),
                completion_area,
                buf,
                &mut self.completion.state,
            );
        }
    }
}

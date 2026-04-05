use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::style::Style;
use ratatui::widgets::Clear;
use ratatui::widgets::List;
use ratatui::widgets::StatefulWidget;
use ratatui::widgets::Widget;

use super::Input;

impl<'a> Input<'a> {
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        self.textarea.render(area, buf);

        let Some(active) = self.completion.active().as_ref() else {
            return;
        };
        let matches = self.completion.matches();
        if !matches.is_empty() {
            let width = matches
                .iter()
                .map(|item| item.match_text.chars().count())
                .max()
                .unwrap_or(0) as u16;
            let height = (matches.len() as u16).min(self.completion.max_height());
            let completion_area = Rect {
                x: area.x + active.request.start as u16,
                y: area.y.saturating_sub(height) + self.textarea.cursor().0 as u16,
                width,
                height,
            };
            Clear.render(completion_area, buf);
            StatefulWidget::render(
                List::new(matches.clone().into_iter().map(|item| item.rendered))
                    .highlight_style(Style::new().reversed()),
                completion_area,
                buf,
                &mut self.completion.state,
            );
        }
    }
}

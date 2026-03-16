use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;

use crate::tui::widgets::container::element::*;

impl<'a> HeightComputable for Paragraph<'a> {
    fn height(
        &mut self,
        width: u16,
        _ctx: RenderContext,
    ) -> u16 {
        Paragraph::line_count(self, width) as u16
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        _ctx: RenderContext,
    ) {
        self.render_ref(area, buf);
    }
}

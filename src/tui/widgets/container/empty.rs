use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::WidgetRef;

use crate::tui::widgets::container::element::*;

#[derive(Debug, Clone)]
pub struct EmptyElement;

impl HeightComputable for EmptyElement {
    fn height(
        &mut self,
        _width: u16,
        _ctx: RenderContext,
    ) -> u16 {
        0
    }

    fn render(
        &mut self,
        _area: Rect,
        _buf: &mut Buffer,
        _ctx: RenderContext,
    ) {
    }
}

impl WidgetRef for EmptyElement {
    fn render_ref(
        &self,
        _area: Rect,
        _buf: &mut Buffer,
    ) {
    }
}

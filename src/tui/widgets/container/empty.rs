use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Block;

use crate::tui::widgets::container::element::HeightComputable;
use crate::tui::widgets::container::element::RenderContext;

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

impl<T> HeightComputable for Option<&mut T>
where T: HeightComputable
{
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        self.as_mut().map_or(0, |v| v.height(width, ctx))
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        if let Some(v) = self {
            v.render(area, buf, ctx);
        }
    }

    fn block(
        &self,
        ctx: RenderContext,
    ) -> Option<Block<'_>> {
        self.as_ref().and_then(|v| v.block(ctx))
    }
}

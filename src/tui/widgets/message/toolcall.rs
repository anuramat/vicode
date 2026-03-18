use derive_more::From;
use ratatui::prelude::*;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;

use crate::llm::message::ToolCallItem;
use crate::tui::widgets::container::element::*;

pub fn style() -> Style {
    Style::default().fg(Color::LightBlue)
}

#[derive(From, Debug, Clone)]
pub struct ToolCallWidget<T>
where T: HeightComputable + Clone
{
    pub name: String,
    pub inner: Option<T>,
}

impl From<&ToolCallItem> for Element {
    fn from(call: &ToolCallItem) -> Self {
        call.task.to_element()
    }
}

impl<T> HeightComputable for ToolCallWidget<T>
where T: HeightComputable + Clone
{
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        if ctx.hide_tools {
            return 1;
        }
        if let Some(inner) = &mut self.inner {
            return inner.height(width, ctx);
        }
        1
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        if !ctx.hide_tools
            && let Some(inner) = &mut self.inner
        {
            return inner.render(area, buf, ctx);
        }
        Paragraph::new(self.name.as_str())
            .style(style().italic())
            .render(area, buf)
    }

    // TODO make method &mut self, and recompute height on call. if height is 1, no block. maybe
    // "collapsed" method, active when inner=None or tools hidden
    fn block(
        &self,
        ctx: RenderContext,
    ) -> Option<Block<'_>> {
        if ctx.hide_tools {
            return None;
        }
        let block = ratatui::widgets::Block::bordered()
            .border_set(ratatui::symbols::border::PLAIN)
            .style(style())
            .title(format!(" {} ", self.name));
        block.into()
    }
}

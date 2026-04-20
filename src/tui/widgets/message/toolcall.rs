use derive_more::From;
use ratatui::prelude::*;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;

use crate::llm::history::message::ToolCallItem;
use crate::tui::colors::TOOLCALL_COLOR;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::container::element::HeightComputable;
use crate::tui::widgets::container::element::RenderContext;

pub fn style() -> Style {
    Style::default().fg(TOOLCALL_COLOR)
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
        2 + self.inner.as_mut().height(width.saturating_sub(2), ctx)
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        if ctx.hide_tools {
            Paragraph::new(self.name.as_str())
                .style(style().italic())
                .render(area, buf);
        } else {
            let block = self.block();
            block.render_ref(area, buf);
            self.inner.as_mut().render(block.inner(area), buf, ctx);
        }
    }
}

impl<T> ToolCallWidget<T>
where T: HeightComputable + Clone
{
    fn block(&self) -> Block<'static> {
        ratatui::widgets::Block::bordered()
            .border_set(ratatui::symbols::border::PLAIN)
            .style(style())
            .title(format!(" {} ", self.name))
    }
}

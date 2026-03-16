use derive_more::From;
use ratatui::prelude::*;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;

use crate::llm::message::ToolCallItem;
use crate::tui::widgets::container::element::*;

pub fn style() -> Style {
    Style::default().fg(Color::LightBlue)
}

#[derive(From, Debug, Clone)]
pub struct ToolCallWidget {
    pub name: String,
    pub inner: Paragraph<'static>,
}

impl From<&ToolCallItem> for Element {
    fn from(call: &ToolCallItem) -> Self {
        call.task.to_element()
    }
}

impl HeightComputable for ToolCallWidget {
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        if ctx.hide_tools {
            return 1;
        }
        self.inner.line_count(width) as u16
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        if !ctx.hide_tools {
            return self.inner.render_ref(area, buf);
        }
        Paragraph::new(self.name.as_str())
            .style(style().italic())
            .render(area, buf)
    }

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

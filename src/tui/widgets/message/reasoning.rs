use ratatui::prelude::*;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;

use crate::llm::message::ReasoningItem;
use crate::tui::widgets::container::element::*;

fn style() -> Style {
    Style::default().fg(Color::LightBlue)
}

#[derive(Debug, Clone)]
struct ReasoningWidget {
    widget: Paragraph<'static>,
    timing: String,
    char_count: usize,
}

impl ReasoningWidget {
    fn title(&self) -> String {
        format!("reasoning: {} chars, {}", self.char_count, self.timing)
    }
}

impl HeightComputable for ReasoningWidget {
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        if ctx.hide_reasoning {
            return 1;
        }
        self.widget.line_count(width) as u16
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        if !ctx.hide_reasoning {
            return self.widget.render_ref(area, buf);
        }
        Paragraph::new(self.title())
            .style(style().italic())
            .render(area, buf)
    }

    fn block(
        &self,
        ctx: RenderContext,
    ) -> Option<Block<'_>> {
        if ctx.hide_reasoning {
            return None;
        }
        Block::bordered()
            .title(format!(" {} ", self.title()))
            .style(style())
            .into()
    }
}

impl From<&ReasoningItem> for Element {
    fn from(item: &ReasoningItem) -> Self {
        let mut text = String::new();
        item.summary.iter().for_each(|s| text.push_str(s));
        if text.is_empty()
            && let Some(content) = &item.content
        {
            content.iter().for_each(|c| text.push_str(c));
        }

        let char_count = text.chars().count();
        let widget = ReasoningWidget {
            widget: Paragraph::new(text)
                .style(style())
                .wrap(Wrap { trim: false }),
            char_count,
            timing: item.timing.to_string(),
        };
        widget.into()
    }
}

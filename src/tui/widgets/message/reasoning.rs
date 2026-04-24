use ratatui::prelude::*;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;

use crate::llm::message::ReasoningItem;
use crate::tui::colors::REASONING_COLOR;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::container::element::HeightComputable;
use crate::tui::widgets::container::element::RenderContext;

fn style() -> Style {
    Style::default().fg(REASONING_COLOR)
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

    fn block(&self) -> Block<'_> {
        Block::bordered()
            .title(format!(" {} ", self.title()))
            .style(style())
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
        self.widget.line_count(width.saturating_sub(2)) as u16 + 2
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        if ctx.hide_reasoning {
            Paragraph::new(self.title())
                .style(style().italic())
                .render(area, buf);
        } else {
            let block = self.block();
            block.render_ref(area, buf);
            self.widget.render_ref(block.inner(area), buf);
        }
    }
}

impl From<&ReasoningItem> for Element {
    fn from(item: &ReasoningItem) -> Self {
        let mut text = String::new();
        item.summary.iter().for_each(|s| text.push_str(s));
        if text.is_empty()
            && let Some(content) = &item.content
        {
            for c in content {
                text.push_str(c);
            }
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

use derive_more::From;
use ratatui::prelude::*;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;

use crate::llm::message::AsMessageText;
use crate::llm::message::DeveloperMessage;
use crate::tui::colors::DEVELOPER_MESSAGE_COLOR;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::container::element::HeightComputable;
use crate::tui::widgets::container::element::RenderContext;

fn style() -> Style {
    Style::default().fg(DEVELOPER_MESSAGE_COLOR)
}

#[derive(From, Debug, Clone)]
struct DeveloperMessageWidget {
    widget: Paragraph<'static>,
    char_count: usize,
}

fn block() -> Block<'static> {
    Block::bordered()
        .title(" developer message ")
        .style(style())
}

impl HeightComputable for DeveloperMessageWidget {
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        if ctx.hide_developer {
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
        if ctx.hide_developer {
            let text = format!("developer: {} chars", self.char_count);
            Paragraph::new(text)
                .style(style().italic())
                .render(area, buf);
        } else {
            let block = block();
            block.render_ref(area, buf);
            self.widget.render_ref(block.inner(area), buf);
        }
    }
}

impl From<&DeveloperMessage> for Element {
    fn from(msg: &DeveloperMessage) -> Self {
        let text = msg.as_message_text();
        let char_count = text.chars().count();
        let widget = DeveloperMessageWidget {
            widget: Paragraph::new(text)
                .style(style())
                .wrap(Wrap { trim: false }),
            char_count,
        };
        widget.into()
    }
}

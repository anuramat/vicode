use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use crate::llm::message::UserMessage;
use crate::tui::widgets::container::element::*;

fn style() -> Style {
    Style::default().fg(Color::Red)
}

impl From<&UserMessage> for Element {
    fn from(value: &UserMessage) -> Self {
        Paragraph::new(value.text.clone())
            .style(style())
            .wrap(Wrap { trim: false })
            .into()
    }
}

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use crate::llm::history::message::UserMessage;
use crate::tui::colors::USER_MESSAGE_COLOR;
use crate::tui::widgets::container::element::Element;

fn style() -> Style {
    Style::default().fg(USER_MESSAGE_COLOR)
}

impl From<&UserMessage> for Element {
    fn from(value: &UserMessage) -> Self {
        Paragraph::new(value.text.clone())
            .style(style())
            .wrap(Wrap { trim: false })
            .into()
    }
}

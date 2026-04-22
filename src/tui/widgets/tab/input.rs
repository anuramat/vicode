use derive_more::Deref;
use derive_more::DerefMut;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols::line::HORIZONTAL;
use ratatui::symbols::line::THICK_HORIZONTAL;

use crate::tui::colors::INPUT_ACTIVE_COLOR;
use crate::tui::colors::INPUT_INACTIVE_COLOR;
use crate::tui::widgets::input::Input;

#[derive(Debug, Clone, Deref, DerefMut)]
pub struct MessageInput<'a> {
    #[deref]
    #[deref_mut]
    pub input: Input<'a>,
    // border between input and messages
    pub title: String,
}

impl MessageInput<'_> {
    pub fn visible(&self) -> bool {
        self.focused() || !self.empty()
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if !self.visible() {
            return;
        }
        let (symbol, color) = if self.focused() {
            (THICK_HORIZONTAL, INPUT_ACTIVE_COLOR)
        } else {
            (HORIZONTAL, INPUT_INACTIVE_COLOR)
        };
        buf.set_string(
            area.x,
            area.y,
            symbol.repeat(area.width.into()),
            Style::default().fg(color),
        );
        buf.set_string(area.x + 1, area.y, &self.title, Style::default());
        self.input.render(
            Rect {
                y: area.y + 1,
                height: area.height.saturating_sub(1),
                ..area
            },
            buf,
        );
    }
}

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::StatefulWidget;
use ratatui::widgets::Widget;
use strum::IntoEnumIterator;

use crate::tui::command::CommandName;
use crate::tui::textarea::Input;

lazy_static::lazy_static! {
    static ref COMMANDS: Vec<String> = CommandName::iter().map(|c| c.to_string()).collect();
}

const MAX_COMPLETION_HEIGHT: u16 = 5;

#[derive(Debug, Clone)]
pub struct Cmdline<'a> {
    pub input: Input<'a>,
}

impl Default for Cmdline<'_> {
    fn default() -> Self {
        Self {
            input: Input::new("", COMMANDS.clone(), MAX_COMPLETION_HEIGHT),
        }
    }
}

impl<'a> Cmdline<'a> {
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let char_area = Rect { width: 1, ..area };
        let textarea_area = Rect {
            x: area.x.saturating_add(char_area.width),
            width: area.width.saturating_sub(char_area.width),
            ..area
        };
        self.input.render(textarea_area, buf);
        Line::raw(":").render(char_area, buf);
    }
}

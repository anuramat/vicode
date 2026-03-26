use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Clear;
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
        let textarea_area = Rect {
            x: area.x.saturating_add(1),
            width: area.width.saturating_sub(1),
            ..area
        };
        buf.set_string(area.x, area.y, ":", ratatui::style::Style::default());
        // WARN might need a Clear render?
        self.input.render(textarea_area, buf);
    }
}

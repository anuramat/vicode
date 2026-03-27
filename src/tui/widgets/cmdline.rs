use anyhow::Result;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::*;
use ratatui::widgets::Clear;
use strum::IntoEnumIterator;

use crate::tui::command::Command;
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
        let mut input = Input::new("", COMMANDS.clone(), MAX_COMPLETION_HEIGHT);
        input.clear_on_unfocus = true;
        Self { input }
    }
}

impl<'a> Cmdline<'a> {
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let height = self.input.textarea.lines().len() as u16;
        let delta = height.saturating_sub(area.height);
        let area = Rect {
            y: area.y.saturating_sub(delta),
            height: area.height + delta,
            ..area
        };
        Clear.render(area, buf);
        buf.set_string(
            area.x,
            area.y,
            ":",
            ratatui::style::Style::default().dark_gray(),
        );
        self.input.render(
            Rect {
                x: area.x + 1,
                y: area.y,
                width: area.width.saturating_sub(1),
                height: area.height,
            },
            buf,
        );
    }

    pub fn take_command(&mut self) -> Result<Command> {
        let area = self.input.take_area();
        let text = area.lines().join("\n");
        let text = text.trim();

        if let Ok(command) = text.parse::<Command>() {
            Ok(command)
        } else if self.input.completion.matches.len() == 1 {
            let text = self.input.completion.matches[0].0.clone();
            Ok(Command {
                name: text.parse().expect("match should be valid command"),
                args: None,
            })
        } else {
            anyhow::bail!("invalid command '{text}'");
        }
    }
}

use anyhow::Result;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::*;
use ratatui::widgets::Clear;
use strum::IntoEnumIterator;

use crate::tui::colors::STL_FG;
use crate::tui::command::Command;
use crate::tui::command::CommandName;
use crate::tui::widgets::input::CompletionItem;
use crate::tui::widgets::input::CompletionSource;
use crate::tui::widgets::input::Input;
use crate::tui::widgets::input::InputOpts;

const MAX_COMPLETION_HEIGHT: u16 = 5;

#[derive(Debug, Clone)]
pub struct Cmdline<'a> {
    pub input: Input<'a>,
}

impl Cmdline<'_> {
    pub fn new() -> Self {
        let input = Input::new(InputOpts {
            source: CompletionSource::Command(
                CommandName::iter()
                    .map(|c| CompletionItem::new(c.to_string()))
                    .collect(),
            ),
            height: MAX_COMPLETION_HEIGHT,
            clear_on_unfocus: true,
        });
        Self { input }
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let height: u16 = self
            .input
            .textarea
            .lines()
            .len()
            .try_into()
            .expect("that's a lot of lines");
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
            ratatui::style::Style::default().fg(STL_FG),
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
        let only_match = self
            .input
            .only_match()
            .map(|only| only.parse().expect("only match should be valid command"));
        self.input.set_focus(false);
        let text = area.lines().join("\n");
        let text = text.trim();

        if let Ok(command) = text.parse::<Command>() {
            Ok(command)
        } else if !text.is_empty()
            && let Some(only) = only_match
        {
            Ok(Command {
                name: only,
                args: None,
            })
        } else {
            anyhow::bail!("invalid command '{text}'");
        }
    }
}

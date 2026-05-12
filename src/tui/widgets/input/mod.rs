pub mod completion;
pub mod keys;
pub mod render;

use std::mem;

use anyhow::Result;
pub use completion::*;
use derive_getters::Getters;
use nucleo_matcher::Matcher;
use ratatui::style::Style;
use ratatui::widgets::ListItem;
use ratatui::widgets::ListState;
use tui_textarea::CursorMove;
use tui_textarea::TextArea;

use crate::llm::history::count_text_tokens;

#[derive(Debug, Clone, Getters)]
pub struct Input<'a> {
    focused: bool,
    clear_on_unfocus: bool,
    pub textarea: TextArea<'a>, // TODO make private
    pub completion: Completion,
}

fn new_textarea(content: &str) -> TextArea<'static> {
    let mut area = TextArea::new(content.split('\n').map(String::from).collect());
    area.set_cursor_line_style(Style::default());
    area
}

pub struct InputOpts {
    pub source: CompletionSource,
    pub height: u16,
    pub clear_on_unfocus: bool,
}

impl<'a> Input<'a> {
    pub fn new(
        InputOpts {
            source,
            height,
            clear_on_unfocus,
        }: InputOpts
    ) -> Self {
        Self {
            clear_on_unfocus,
            completion: Completion::new(height, source),
            textarea: new_textarea(""),
            focused: false,
        }
    }

    pub fn set_focus(
        &mut self,
        focus: bool,
    ) {
        self.focused = focus;
        if (!focus && self.clear_on_unfocus) || self.text().chars().all(char::is_whitespace) {
            self.textarea = new_textarea("");
        }
        self.clear_completion();
    }

    pub fn take_area(&mut self) -> TextArea<'a> {
        let mut empty = new_textarea("");
        mem::swap(&mut self.textarea, &mut empty);
        empty
    }

    pub fn prepend_text(
        &mut self,
        mut text: String,
    ) {
        let (row_from_bottom, col) = {
            let (row, col) = self.textarea.cursor();
            (self.textarea.lines().len() - row, col)
        };

        text.push('\n');

        let current = self.take_area().lines().join("\n");
        text.push_str(&current);

        self.textarea = {
            let mut area = new_textarea(&text);
            let n_lines = area.lines().len() as u16;
            let row = n_lines.saturating_sub(row_from_bottom as u16);
            area.move_cursor(CursorMove::Jump(row, col as u16));
            area
        };
    }

    pub fn empty(&self) -> bool {
        let lines = self.textarea.lines();
        lines.len() == 1 && lines[0].is_empty()
    }

    fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn count_tokens(&self) -> usize {
        count_text_tokens(&self.text())
    }

    pub(super) fn line_until_cursor(&self) -> String {
        let (row, col) = self.textarea.cursor();
        self.textarea
            .lines()
            .get(row)
            .cloned()
            .unwrap_or_default()
            .chars()
            .take(col)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;
    use tui_textarea::CursorMove;

    use super::*;

    fn input(
        text: &str,
        source: CompletionSource,
    ) -> Input<'static> {
        let mut input = Input::new(InputOpts {
            source,
            height: 5,
            clear_on_unfocus: false,
        });
        input.textarea.insert_str(text);
        input.textarea.move_cursor(CursorMove::End);
        input.completion_update();
        input
    }

    #[test]
    fn leading_word_only_matches_at_column_zero() {
        let input = input(
            "compact foo",
            CompletionSource::Command(vec![CompletionItem::new("compact".into())]),
        );

        assert_eq!(input.completion.items().len(), 0);
    }

    #[test]
    fn cancel_restores_typed_prefix() {
        let mut input = input(
            "open @sr",
            CompletionSource::Freeform(vec![(
                '@',
                vec![CompletionItem::new("@src/main.rs".into())],
            )]),
        );

        input.completion_next();
        input.completion_cancel();

        assert_eq!(input.textarea.lines(), ["open @sr"]);
    }

    #[test]
    fn freeform_requires_prefix() {
        let input = input(
            "open sr",
            CompletionSource::Freeform(vec![(
                '@',
                vec![CompletionItem::new("@src/main.rs".into())],
            )]),
        );

        assert_eq!(input.completion.items(), &Vec::<CompletionItem>::new());
    }
}

pub mod completion;
pub mod keys;
pub mod render;

use std::mem;

use anyhow::Result;
pub use completion::*;
use derive_getters::Getters;
use nucleo_matcher::Matcher;
use ratatui::widgets::ListItem;
use ratatui::widgets::ListState;
use tui_textarea::CursorMove;
use tui_textarea::TextArea;

use crate::llm::tokens::count_text_tokens;

#[derive(Debug, Clone, Getters)]
pub struct Input<'a> {
    focused: bool,
    clear_on_unfocus: bool,
    pub textarea: TextArea<'a>, // TODO make private
    pub completion: Completion<'a>,
}

fn new_textarea(content: &str) -> TextArea<'static> {
    let mut area = TextArea::new(content.split('\n').map(String::from).collect());
    area.set_cursor_line_style(Default::default());
    area
}

pub struct InputOpts<'a> {
    pub source: Vec<CompletionItem<'a>>,
    pub height: u16,
    pub clear_on_unfocus: bool,
    pub only_leading: bool,
}

impl<'a> Input<'a> {
    pub fn new(
        InputOpts {
            source,
            height,
            clear_on_unfocus,
            only_leading,
        }: InputOpts<'a>
    ) -> Self {
        Self {
            clear_on_unfocus,
            completion: Completion::new(height, source, only_leading),
            textarea: new_textarea(""),
            focused: false,
        }
    }

    pub fn get_mut(&mut self) -> Result<&mut Input<'a>> {
        if self.focused {
            Ok(self)
        } else {
            anyhow::bail!("cannot get textarea when input is not focused")
        }
    }

    pub fn set_focus(
        &mut self,
        focus: bool,
    ) {
        self.focused = focus;
        if !focus {
            if self.clear_on_unfocus || self.text().chars().all(|c| c.is_whitespace()) {
                self.textarea = new_textarea("");
            }
            self.clear_completion();
        }
    }

    pub fn take_area(&mut self) -> TextArea<'a> {
        let mut empty = new_textarea("");
        mem::swap(&mut self.textarea, &mut empty);
        self.set_focus(false);
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
    // use similar_asserts::assert_eq;
    // use tui_textarea::CursorMove;
    //
    // use super::*;

    // #[test]
    // fn leading_word_only_matches_at_column_zero() {
    //     let mut input = Input::new(
    //         "compact foo",
    //         CompletionSource::leading_word(vec![CompletionItem::plain("compact".into())]),
    //         5,
    //     );
    //     input.textarea.move_cursor(CursorMove::End);
    //     input.handle_completion();
    //     assert_eq!(input.completion_matches().len(), 0);
    // }

    // #[test]
    // fn prefixed_word_matches_without_prefix_in_entries() {
    //     let mut input = Input::new(
    //         "open @sr",
    //         CompletionSource::prefixed_word(
    //             '@',
    //             vec![CompletionItem {
    //                 match_text: "src/main.rs".into(),
    //                 insert_text: "@src/main.rs".into(),
    //                 rendered: ListItem::new("src/main.rs"),
    //             }],
    //         ),
    //         5,
    //     );
    //     input.textarea.move_cursor(CursorMove::End);
    //     input.handle_completion();
    //
    //     assert_eq!(
    //         input
    //             .completion_matches()
    //             .iter()
    //             .map(|item| item.match_text.clone())
    //             .collect::<Vec<_>>(),
    //         vec!["src/main.rs".to_string()]
    //     );
    // }
    //
    // #[test]
    // fn cancel_restores_typed_prefix() {
    //     let mut input = Input::new(
    //         "open @sr",
    //         CompletionSource::prefixed_word(
    //             '@',
    //             vec![CompletionItem {
    //                 match_text: "src/main.rs".into(),
    //                 insert_text: "@src/main.rs".into(),
    //                 rendered: ListItem::new("src/main.rs"),
    //             }],
    //         ),
    //         5,
    //     );
    //     input.textarea.move_cursor(CursorMove::End);
    //     input.handle_completion();
    //     input.completion_next();
    //     input.completion_cancel();
    //
    //     assert_eq!(input.textarea.lines(), ["open @sr"]);
    // }
    //
    // #[test]
    // fn updating_source_items_refreshes_active_matches() {
    //     let mut input = Input::new("open @sr", CompletionSource::prefixed_word('@', vec![]), 5);
    //     input.textarea.move_cursor(CursorMove::End);
    //     input.set_completion_items(vec![CompletionItem {
    //         match_text: "src/main.rs".into(),
    //         insert_text: "@src/main.rs".into(),
    //         rendered: ListItem::new("src/main.rs"),
    //     }]);
    //
    //     assert_eq!(
    //         input
    //             .completion_matches()
    //             .iter()
    //             .map(|item| item.match_text.clone())
    //             .collect::<Vec<_>>(),
    //         vec!["src/main.rs".to_string()]
    //     );
    // }
}

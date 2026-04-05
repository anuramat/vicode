pub mod completion;
pub mod keys;
pub mod render;

use std::mem;
use std::sync::Arc;

pub use completion::*;
use nucleo_matcher::Matcher;
use ratatui::widgets::ListItem;
use ratatui::widgets::ListState;
use tui_textarea::CursorMove;
use tui_textarea::TextArea;

type CompletionExtract = Arc<dyn Fn(&str) -> Option<CompletionRequest> + Send + Sync>;

#[derive(Debug, Clone)]
pub struct Input<'a> {
    pub focus: bool,
    pub clear_on_unfocus: bool,
    pub textarea: TextArea<'a>,
    pub(super) completion: Completion<'a>,
}

fn new_area(content: &str) -> TextArea<'static> {
    let mut area = TextArea::new(content.split('\n').map(String::from).collect());
    area.set_cursor_line_style(Default::default());
    area
}

impl<'a> Input<'a> {
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
            let mut area = new_area(&text);
            let n_lines = area.lines().len() as u16;
            let row = n_lines.saturating_sub(row_from_bottom as u16);
            area.move_cursor(CursorMove::Jump(row, col as u16));
            area
        };
    }

    pub fn take_area(&mut self) -> TextArea<'a> {
        let mut empty = new_area("");
        mem::swap(&mut self.textarea, &mut empty);
        self.focus = false;
        empty
    }

    pub fn focus(
        &mut self,
        value: bool,
    ) {
        self.focus = value;
        if !value {
            self.clear_completion();
            if self.clear_on_unfocus {
                self.textarea = new_area("");
            }
        }
    }

    pub fn line(&self) -> String {
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

    pub fn new(
        contents: &str,
        completion_sources: Vec<CompletionSource<'a>>,
        completion_max_height: u16,
    ) -> Self {
        Self {
            clear_on_unfocus: false,
            focus: false,
            textarea: new_area(contents),
            completion: Completion::new(completion_max_height, completion_sources),
        }
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;
    use tui_textarea::CursorMove;

    use super::*;

    #[test]
    fn leading_word_only_matches_at_column_zero() {
        let mut input = Input::new(
            "compact foo",
            vec![CompletionSource::leading_word(
                "commands",
                vec![CompletionItem::plain("compact".into())],
            )],
            5,
        );
        input.textarea.move_cursor(CursorMove::End);
        input.handle_completion();
        assert_eq!(input.completion_matches().len(), 0);
    }

    #[test]
    fn prefixed_word_matches_without_prefix_in_entries() {
        let mut input = Input::new(
            "open @sr",
            vec![CompletionSource::prefixed_word(
                "files",
                '@',
                vec![CompletionItem {
                    match_text: "src/main.rs".into(),
                    insert_text: "@src/main.rs".into(),
                    rendered: ListItem::new("src/main.rs"),
                }],
            )],
            5,
        );
        input.textarea.move_cursor(CursorMove::End);
        input.handle_completion();

        assert_eq!(
            input
                .completion_matches()
                .iter()
                .map(|item| item.match_text.clone())
                .collect::<Vec<_>>(),
            vec!["src/main.rs".to_string()]
        );
    }

    #[test]
    fn cancel_restores_typed_prefix() {
        let mut input = Input::new(
            "open @sr",
            vec![CompletionSource::prefixed_word(
                "files",
                '@',
                vec![CompletionItem {
                    match_text: "src/main.rs".into(),
                    insert_text: "@src/main.rs".into(),
                    rendered: ListItem::new("src/main.rs"),
                }],
            )],
            5,
        );
        input.textarea.move_cursor(CursorMove::End);
        input.handle_completion();
        input.completion_next();
        input.completion_cancel();

        assert_eq!(input.textarea.lines(), ["open @sr"]);
    }

    #[test]
    fn updating_source_items_refreshes_active_matches() {
        let mut input = Input::new(
            "open @sr",
            vec![CompletionSource::prefixed_word("files", '@', vec![])],
            5,
        );
        input.textarea.move_cursor(CursorMove::End);
        input.set_completion_items(
            "files",
            vec![CompletionItem {
                match_text: "src/main.rs".into(),
                insert_text: "@src/main.rs".into(),
                rendered: ListItem::new("src/main.rs"),
            }],
        );

        assert_eq!(
            input
                .completion_matches()
                .iter()
                .map(|item| item.match_text.clone())
                .collect::<Vec<_>>(),
            vec!["src/main.rs".to_string()]
        );
    }
}

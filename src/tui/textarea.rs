use std::mem;

// TODO rename file
// TODO rename command stuff to generic
use crossterm::event::KeyCode;
use crossterm::event::KeyCode::Char;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers as Mods;
use nucleo_matcher::Matcher;
use nucleo_matcher::pattern::Atom;
use nucleo_matcher::pattern::AtomKind;
use nucleo_matcher::pattern::CaseMatching;
use nucleo_matcher::pattern::Normalization;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::style::Style;
use ratatui::widgets::Clear;
use ratatui::widgets::List;
use ratatui::widgets::ListItem;
use ratatui::widgets::ListState;
use ratatui::widgets::StatefulWidget;
use ratatui::widgets::Widget;
use tui_textarea::CursorMove;
use tui_textarea::CursorMove::*;
use tui_textarea::TextArea;

#[derive(Debug, Clone, Default)]
pub struct Completion<'a> {
    items: Vec<Item<'a>>,
    pub matches: Vec<Item<'a>>,
    state: ListState,
    prefix: String,
    max_height: u16,
    matcher: Matcher,
}

#[derive(Debug, Clone)]
pub struct Item<'a> {
    pub value: String,
    pub rendered: ListItem<'a>,
}

impl AsRef<str> for Item<'_> {
    fn as_ref(&self) -> &str {
        self.value.as_str()
    }
}

#[derive(Debug, Clone)]
pub struct Input<'a> {
    pub focus: bool,
    pub clear_on_unfocus: bool,
    pub textarea: TextArea<'a>,
    pub completion: Completion<'a>,
}

fn prefix(text: &str) -> (usize, &str) {
    text.rsplit_once(' ')
        .map(|(head, tail)| (head.len() + 1, tail))
        .unwrap_or((0, text))
}

fn new_area() -> TextArea<'static> {
    let mut area = TextArea::default();
    area.set_cursor_line_style(Default::default());
    area
}

fn new_area_from_str(contents: String) -> TextArea<'static> {
    let lines: Vec<String> = contents.split('\n').map(String::from).collect();
    let mut area = TextArea::new(lines);
    area.set_cursor_line_style(Default::default());
    area
}

impl<'a> Input<'a> {
    pub fn prepend_text(
        &mut self,
        mut text: String,
    ) {
        let (row, col) = self.textarea.cursor();
        let current = self.take_area().lines().join("\n");
        let row_offset = text.matches('\n').count() as u16; // XXX test this
        text.push_str(&current);
        self.textarea = new_area_from_str(text);
        self.textarea
            .move_cursor(CursorMove::Jump(row as u16 + row_offset, col as u16));
        self.textarea.set_cursor_line_style(Default::default());
    }

    pub fn take_area(&mut self) -> TextArea<'a> {
        let mut empty = new_area();
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
            self.completion.state.select(None);
            self.completion.matches.clear();
            if self.clear_on_unfocus {
                self.textarea = new_area();
            }
        }
    }

    // PERF use a bitset for matches?
    // TODO fuzzy match

    pub fn narrow(&mut self) {
        let matches = Atom::new(
            &self.completion.prefix,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
            false,
        )
        .match_list(&self.completion.matches, &mut self.completion.matcher);
        self.completion.matches = matches.into_iter().map(|item| item.0.clone()).collect();
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

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        self.textarea.render(area, buf);

        let prefix_column = prefix(&self.line()).0;

        let matches = &mut self.completion.matches;
        if !matches.is_empty() {
            let width = matches
                .iter()
                .map(|item| item.value.len())
                .max()
                .unwrap_or(0) as u16;
            let height = (matches.len() as u16).min(self.completion.max_height);
            let completion_area = Rect {
                x: area.x + prefix_column as u16,
                y: area.y.saturating_sub(height) + self.textarea.cursor().0 as u16, // TEST this
                width,
                height,
            };
            Clear.render(completion_area, buf);
            StatefulWidget::render(
                // PERF store list in struct and use render_ref
                List::new(matches.clone().into_iter().map(|item| item.rendered))
                    .highlight_style(Style::new().reversed()),
                completion_area,
                buf,
                &mut self.completion.state,
            );
        }
    }

    fn completion_accept(&mut self) {
        if let Some(item) = self
            .completion
            .matches
            .get(self.completion.state.selected().unwrap_or(0))
        {
            let line: String = self.line();
            let last_word = prefix(&line).1;
            for _ in 0..last_word.len() {
                self.textarea.delete_char();
            }
            self.textarea.insert_str(item.value.clone());
        }
    }

    pub fn completion_cancel(&mut self) {
        let line: String = self.line();
        let last_word = prefix(&line).1;
        for _ in 0..last_word.len() {
            self.textarea.delete_char();
        }
        self.textarea.insert_str(self.completion.prefix.as_str());
    }

    fn init_if_empty(&mut self) {
        if self.completion.prefix.is_empty() && self.completion.matches.is_empty() {
            self.completion.matches = self.completion.items.clone();
            self.completion.state.select(None);
        }
    }

    pub fn completion_next(&mut self) {
        self.init_if_empty();
        if self.completion.state.selected().is_none() {
            self.completion.state.select(Some(0));
        } else {
            self.completion.state.select_next();
        }
        self.completion_accept();
    }

    pub fn completion_prev(&mut self) {
        self.init_if_empty();
        if self.completion.state.selected().is_none() {
            self.completion
                .state
                .select(Some(self.completion.items.len().saturating_sub(1)));
        } else {
            self.completion.state.select_previous();
        }
        self.completion_accept();
    }

    pub fn new(
        contents: &str,
        completion_items: Vec<String>,
        completion_max_height: u16,
    ) -> Self {
        let mut area = TextArea::new(contents.split('\n').map(String::from).collect());
        area.set_cursor_line_style(Default::default());
        Self {
            clear_on_unfocus: false,
            focus: false,
            textarea: area,
            completion: Completion {
                matcher: Matcher::default(),
                max_height: completion_max_height,
                matches: Vec::new(),
                prefix: String::new(),
                items: completion_items
                    .into_iter()
                    .map(|command| Item {
                        value: command.clone(),
                        rendered: ListItem::new(command),
                    })
                    .collect(),
                state: ListState::default(),
            },
        }
    }

    pub fn handle_completion(&mut self) {
        let line: String = self.line();
        let (_, new_prefix) = prefix(&line);
        if new_prefix.is_empty() {
            self.completion.matches.clear();
            self.completion.prefix.clear();
            self.completion.state.select(None);
            return;
        }
        if new_prefix == self.completion.prefix {
            return;
        }
        if !new_prefix.starts_with(&self.completion.prefix) || self.completion.prefix.is_empty() {
            self.completion.matches = self.completion.items.clone();
        }
        self.completion.prefix = new_prefix.to_string();
        self.narrow();
        self.completion.state.select(None);
    }

    pub fn handle(
        &mut self,
        input: KeyEvent,
    ) {
        let KeyEvent {
            code,
            modifiers: mods,
            ..
        } = input;
        // TODO check if we have all reasonable shortcuts
        // TODO maybe make these configurable? not really important but...
        match code {
            // move:
            Char('a') if mods == Mods::CONTROL => {
                self.textarea.move_cursor(Head);
            }
            Char('e') if mods == Mods::CONTROL => {
                self.textarea.move_cursor(End);
            }
            Char('b') if mods == Mods::ALT => {
                self.textarea.move_cursor(WordBack);
            }
            Char('f') if mods == Mods::ALT => {
                self.textarea.move_cursor(WordForward);
            }
            Char('b') if mods == Mods::CONTROL => {
                self.textarea.move_cursor(Back);
            }
            Char('f') if mods == Mods::CONTROL => {
                self.textarea.move_cursor(Forward);
            }

            // delete:
            Char('u') if mods == Mods::CONTROL => {
                self.textarea.delete_line_by_head();
            }
            Char('k') if mods == Mods::CONTROL => {
                self.textarea.delete_line_by_end();
            }
            Char('w') if mods == Mods::CONTROL => {
                // TODO 'WORD', not 'word'
                self.textarea.delete_word();
            }
            Char('d') if mods == Mods::CONTROL | Mods::ALT => {
                // TODO 'WORD', not 'word'
                self.textarea.delete_word();
            }
            KeyCode::Backspace if mods == Mods::ALT => {
                self.textarea.delete_word();
            }
            Char('d') if mods == Mods::ALT => {
                self.textarea.delete_next_word();
            }
            Char('h') if mods == Mods::CONTROL => {
                self.textarea.delete_char();
            }
            Char('d') if mods == Mods::CONTROL => {
                self.textarea.delete_next_char();
            }
            _ => _ = self.textarea.input_without_shortcuts(input),
        }
        self.handle_completion();
    }
}

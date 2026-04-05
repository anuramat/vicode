use crossterm::event::KeyCode;
use crossterm::event::KeyCode::Char;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers as Mods;
use tui_textarea::CursorMove::*;

use super::Input;

impl<'a> Input<'a> {
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

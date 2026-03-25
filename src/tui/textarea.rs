use crossterm::event::KeyCode;
use crossterm::event::KeyCode::Char;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers as Mods;
use tui_textarea::CursorMove::*;
use tui_textarea::TextArea;

pub fn new<'a>() -> TextArea<'a> {
    from_str("")
}

pub fn from_str<'a>(s: &str) -> TextArea<'a> {
    let mut area = TextArea::new(s.split('\n').map(String::from).collect());
    area.insert_str(s);
    area.set_cursor_line_style(Default::default());
    area
}

pub fn handle(
    area: &mut TextArea<'_>,
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
            area.move_cursor(Head);
        }
        Char('e') if mods == Mods::CONTROL => {
            area.move_cursor(End);
        }
        Char('b') if mods == Mods::ALT => {
            area.move_cursor(WordBack);
        }
        Char('f') if mods == Mods::ALT => {
            area.move_cursor(WordForward);
        }
        Char('b') if mods == Mods::CONTROL => {
            area.move_cursor(Back);
        }
        Char('f') if mods == Mods::CONTROL => {
            area.move_cursor(Forward);
        }

        // delete:
        Char('u') if mods == Mods::CONTROL => {
            area.delete_line_by_head();
        }
        Char('k') if mods == Mods::CONTROL => {
            area.delete_line_by_end();
        }
        Char('w') if mods == Mods::CONTROL => {
            // TODO 'WORD', not 'word'
            area.delete_word();
        }
        Char('d') if mods == Mods::CONTROL | Mods::ALT => {
            // TODO 'WORD', not 'word'
            area.delete_word();
        }
        KeyCode::Backspace if mods == Mods::ALT => {
            area.delete_word();
        }
        Char('d') if mods == Mods::ALT => {
            area.delete_next_word();
        }
        Char('h') if mods == Mods::CONTROL => {
            area.delete_char();
        }
        Char('d') if mods == Mods::CONTROL => {
            area.delete_next_char();
        }
        _ => _ = area.input_without_shortcuts(input),
    }
}

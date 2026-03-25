use std::str::FromStr;

use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use indexmap::IndexMap;
use serde::Deserialize;
use serde_plain::derive_deserialize_from_fromstr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandName {
    AssistantNext,
    CmdlineEnter,
    InputExit,
    InputSubmit,
    InsertEnter,
    Quit,
    ScrollBottom,
    ScrollHalfPageDown,
    ScrollHalfPageUp,
    ScrollLineDown,
    ScrollLineUp,
    ScrollNextElement,
    ScrollPageDown,
    ScrollPageUp,
    ScrollPrevElement,
    ScrollTop,
    SetMultiplier,
    TabDelete,
    TabDuplicate,
    TabNew,
    TabNext,
    TabPrev,
    ToggleDeveloper,
    ToggleReasoning,
    ToggleTools,
    TurnAbort,
    TurnRetry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub name: CommandName,
    pub args: Option<String>,
}

derive_deserialize_from_fromstr!(Command, "valid command");
impl FromStr for Command {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let (name, args) = match s.split_once(' ') {
            Some((name, args)) => {
                if args.is_empty() {
                    (name, None)
                } else {
                    (name, Some(args.to_string()))
                }
            }
            None => (s, None),
        };
        Ok(Command {
            name: serde_plain::from_str(name)?,
            args,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl From<KeyEvent> for KeyChord {
    fn from(value: KeyEvent) -> Self {
        let KeyEvent {
            mut code,
            mut modifiers,
            ..
        } = value;
        if let KeyCode::Char(c) = code
            && c.is_ascii_uppercase()
        {
            code = KeyCode::Char(c.to_ascii_lowercase());
            modifiers |= KeyModifiers::SHIFT;
        }
        Self { code, modifiers }
    }
}

derive_deserialize_from_fromstr!(KeyChord, "valid key chord");
impl FromStr for KeyChord {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let mut parts = s.rsplit('-');
        let code = match parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("empty keybinding"))?
        {
            "enter" => KeyCode::Enter,
            "esc" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "" => anyhow::bail!("empty keybinding"),
            key if key.len() == 1 && !key.chars().any(|c| c.is_ascii_uppercase()) => {
                KeyCode::Char(key.chars().next().unwrap())
            }
            key => anyhow::bail!("invalid key '{key}' in keybinding '{s}'"),
        };
        let modifiers = {
            let mut modifiers = KeyModifiers::empty();
            for part in parts {
                modifiers |= match part {
                    "C" => KeyModifiers::CONTROL,
                    "A" => KeyModifiers::ALT,
                    "S" => KeyModifiers::SHIFT,
                    _ => anyhow::bail!("unknown modifier '{part}' in keybinding '{s}'"),
                }
            }
            modifiers
        };
        Ok(Self { code, modifiers })
    }
}

// TODO allow multiple chords per command
// TODO allow multiple commands per chord
// TODO allow sequences of chords?
// TODO allow easily defining keymap for multiple modes at the same time

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Keymap {
    pub cmdline: IndexMap<KeyChord, Command>,
    pub normal: IndexMap<KeyChord, Command>,
    pub insert: IndexMap<KeyChord, Command>,
}

impl Keymap {
    pub fn cmdline(
        &self,
        event: KeyEvent,
    ) -> Option<Command> {
        self.cmdline.get(&KeyChord::from(event)).cloned()
    }

    pub fn normal(
        &self,
        event: KeyEvent,
    ) -> Option<Command> {
        self.normal.get(&KeyChord::from(event)).cloned()
    }

    pub fn insert(
        &self,
        event: KeyEvent,
    ) -> Option<Command> {
        self.insert.get(&KeyChord::from(event)).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shifted_char_key() {
        let chord: KeyChord = "S-n".parse().unwrap();
        assert_eq!(chord.code, KeyCode::Char('n'));
        assert_eq!(chord.modifiers, KeyModifiers::SHIFT);
    }

    #[test]
    fn normalizes_shifted_char_event() {
        let chord = KeyChord::from(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT));
        assert_eq!(chord.code, KeyCode::Char('n'));
        assert_eq!(chord.modifiers, KeyModifiers::SHIFT);
    }

    #[test]
    fn rejects_uppercase_key_names() {
        let err = "Enter".parse::<KeyChord>().unwrap_err();
        assert!(err.to_string().contains("uppercase key 'Enter'"));

        let err = "D".parse::<KeyChord>().unwrap_err();
        assert!(err.to_string().contains("uppercase key 'D'"));
    }
}

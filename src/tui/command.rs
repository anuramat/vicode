use std::str::FromStr;

use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use serde_plain::derive_deserialize_from_fromstr;
use strum::EnumIter;

// TODO expose usage in completion menu using https://docs.rs/strum/latest/strum/derive.EnumMessage.html

serde_plain::derive_display_from_serialize!(CommandName);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, EnumIter)]
#[serde(rename_all = "snake_case")]
pub enum CommandName {
    AssistantNext,
    CmdlineEnter,
    CompletionCancel,
    CompletionNext,
    CompletionPrev,
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
    ToggleMarkdown,
    ToggleReasoning,
    ToggleTools,
    TurnAbort,
    TurnRetry,
    /// dummy command to unmap keys
    None,
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
        let s = s.to_ascii_lowercase();
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
            key if key.len() == 1 => KeyCode::Char(key.chars().next().unwrap()),
            key => anyhow::bail!("invalid key '{key}' in keybinding '{s}'"),
        };
        let modifiers = {
            let mut modifiers = KeyModifiers::empty();
            for part in parts {
                modifiers |= match part {
                    "c" => KeyModifiers::CONTROL,
                    "a" => KeyModifiers::ALT,
                    "s" => KeyModifiers::SHIFT,
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

#[derive(Debug, Clone, Deserialize)]
pub struct Keymap {
    #[serde(default)]
    pub cmdline: IndexMap<KeyChord, Command>,
    #[serde(default)]
    pub normal: IndexMap<KeyChord, Command>,
    #[serde(default)]
    pub insert: IndexMap<KeyChord, Command>,
}

pub enum Mode {
    Normal,
    Insert,
    Cmdline,
}

impl Keymap {
    pub fn get(
        &self,
        mode: Mode,
        event: KeyEvent,
    ) -> Option<Command> {
        match mode {
            Mode::Cmdline => &self.cmdline,
            Mode::Normal => &self.normal,
            Mode::Insert => &self.insert,
        }
        .get(&KeyChord::from(event))
        .and_then(|c| {
            if c.name == CommandName::None {
                None
            } else {
                Some(c)
            }
        })
        .cloned()
    }

    pub fn cmdline(
        &self,
        event: KeyEvent,
    ) -> Option<Command> {
        self.get(Mode::Cmdline, event)
    }

    pub fn normal(
        &self,
        event: KeyEvent,
    ) -> Option<Command> {
        self.get(Mode::Normal, event)
    }

    pub fn insert(
        &self,
        event: KeyEvent,
    ) -> Option<Command> {
        self.get(Mode::Insert, event)
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
    fn command_names_display_in_config_format() {
        assert_eq!(CommandName::CompletionNext.to_string(), "completion_next");
    }
}

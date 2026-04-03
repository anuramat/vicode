use std::collections::HashMap;
use std::str::FromStr;

use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use schemars::JsonSchema;
use schemars::json_schema;
use serde::Deserialize;
use serde::Serialize;
use serde_plain::derive_deserialize_from_fromstr;
use serde_plain::derive_serialize_from_display;
use strum::EnumIter;

// TODO expose usage in completion menu using https://docs.rs/strum/latest/strum/derive.EnumMessage.html

serde_plain::derive_display_from_serialize!(CommandName);
serde_plain::derive_fromstr_from_deserialize!(CommandName);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, EnumIter, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandName {
    /// select next assistant in the currently selected tab
    AssistantNext,
    /// select previous assistant in the currently selected tab
    AssistantPrev,
    /// switch to cmdline
    CmdlineEnter,
    /// compact the conversation
    Compact,
    /// cancel completion and hide the menu
    CompletionCancel,
    /// select next completion result
    CompletionNext,
    /// select previous completion result
    CompletionPrev,
    /// exit insert mode/cmdline
    InputExit,
    /// submit the message/command
    InputSubmit,
    /// switch to insert mode
    InsertEnter,
    /// paste args into focused input
    InsertPaste,
    /// remove last message
    MsgUndo,
    /// remove messages starting from last user message
    MsgUndoUser,
    /// quit the app
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
    /// set multiplier for the next prompt
    SetMultiplier,
    /// delete the currently selected tab
    TabDelete,
    /// duplicate the currently selected tab
    TabDuplicate,
    /// open new tab
    TabNew,
    /// switch to the next tab
    TabNext,
    /// switch to the previous tab
    TabPrev,
    /// select tab by index
    TabSelect,
    /// show/hide developer messages
    ToggleDeveloper,
    /// toggle markdown rendering
    ToggleMarkdown,
    /// show/hide CoT
    ToggleReasoning,
    /// show/hide tab pane
    ToggleTabs,
    /// show/hide tool calls
    ToggleTools,
    /// abort current turn
    TurnAbort,
    /// retry turn/compact
    TurnRetry,
    /// dummy command to unmap keys
    #[serde(alias = "")]
    None,
}

derive_deserialize_from_fromstr!(Command, "valid command");
derive_serialize_from_display!(Command);
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
pub struct Command {
    pub name: CommandName,
    pub args: Option<String>,
}

impl std::fmt::Display for Command {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        if let Some(args) = &self.args {
            write!(f, "{} {args}", self.name)
        } else {
            write!(f, "{}", self.name)
        }
    }
}

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

derive_deserialize_from_fromstr!(KeyChord, "valid key chord");
derive_serialize_from_display!(KeyChord);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl JsonSchema for KeyChord {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        // Exclude the module path to make the name in generated schemas clearer.
        "KeyChord".into()
    }

    fn json_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        json_schema!({
            "type": "string",
            "description": "A key chord, consisting of a key code and optional modifiers (c- for control, s- for shift, a- for alt).",
            "examples": ["c-a", "s-enter", "tab"],
        })
    }
}

impl From<KeyEvent> for KeyChord {
    fn from(value: KeyEvent) -> Self {
        let KeyEvent {
            mut code,
            mut modifiers,
            ..
        } = value;
        match code {
            KeyCode::Char(c) if c.is_ascii_uppercase() => {
                code = KeyCode::Char(c.to_ascii_lowercase());
                modifiers |= KeyModifiers::SHIFT;
            }
            KeyCode::BackTab => {
                code = KeyCode::Tab;
                modifiers |= KeyModifiers::SHIFT;
            }
            _ => {}
        }
        Self { code, modifiers }
    }
}

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
                    "s" => KeyModifiers::SHIFT,
                    "a" => KeyModifiers::ALT,
                    _ => anyhow::bail!("unknown modifier '{part}' in keybinding '{s}'"),
                }
            }
            modifiers
        };
        Ok(Self { code, modifiers })
    }
}

impl std::fmt::Display for KeyChord {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            write!(f, "c-")?;
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            write!(f, "s-")?;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            write!(f, "a-")?;
        }
        match self.code {
            KeyCode::Enter => write!(f, "enter"),
            KeyCode::Esc => write!(f, "esc"),
            KeyCode::Tab => write!(f, "tab"),
            KeyCode::Backspace => write!(f, "backspace"),
            KeyCode::Up => write!(f, "up"),
            KeyCode::Down => write!(f, "down"),
            KeyCode::Left => write!(f, "left"),
            KeyCode::Right => write!(f, "right"),
            KeyCode::Char(c) => write!(f, "{c}"),
            _ => Err(std::fmt::Error), // TODO
        }
    }
}

// TODO allow multiple commands per chord
// TODO allow easily defining keymap for multiple modes at the same time

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(default)]
pub struct Keymap {
    /// if false, keymaps are merged with defaults
    pub clear_defaults: bool,
    pub cmdline: HashMap<KeyChord, Command>,
    pub normal: HashMap<KeyChord, Command>,
    pub insert: HashMap<KeyChord, Command>,
}

impl Default for Keymap {
    fn default() -> Self {
        fn parse<'a, I>(x: I) -> HashMap<KeyChord, Command>
        where I: IntoIterator<Item = (&'a str, &'a str)> {
            x.into_iter()
                .map(|(k, v)| (k.parse().unwrap(), v.parse().unwrap()))
                .collect()
        }

        let cmdline = [
            ("enter", "input_submit"),
            ("esc", "input_exit"),
            ("c-n", "completion_next"),
            ("c-p", "completion_prev"),
            ("c-e", "completion_cancel"),
        ];

        let normal = [
            (":", "cmdline_enter"),
            ("j", "tab_next"),
            ("k", "tab_prev"),
            ("s-d", "tab_delete"),
            ("s-y", "tab_duplicate"),
            ("o", "tab_new"),
            ("s-q", "quit"),
            ("s-r", "turn_retry"),
            ("s-x", "turn_abort"),
            ("i", "insert_enter"),
            ("u", "msg_undo"),
            ("s-u", "msg_undo_user"),
            ("tab", "assistant_next"),
            ("s-tab", "assistant_prev"),
            ("up", "scroll_line_up"),
            ("down", "scroll_line_down"),
            ("c-y", "scroll_line_up"),
            ("c-e", "scroll_line_down"),
            ("c-u", "scroll_half_page_up"),
            ("c-d", "scroll_half_page_down"),
            ("c-b", "scroll_page_up"),
            ("c-f", "scroll_page_down"),
            ("[", "scroll_prev_element"),
            ("]", "scroll_next_element"),
            ("g", "scroll_top"),
            ("s-g", "scroll_bottom"),
            ("1", "set_multiplier 1"),
            ("2", "set_multiplier 2"),
            ("3", "set_multiplier 3"),
            ("4", "set_multiplier 4"),
            ("5", "set_multiplier 5"),
            ("6", "set_multiplier 6"),
            ("7", "set_multiplier 7"),
            ("8", "set_multiplier 8"),
            ("9", "set_multiplier 9"),
        ];

        let insert = [("enter", "input_submit"), ("esc", "input_exit")];
        Self {
            clear_defaults: false,
            cmdline: parse(cmdline),
            normal: parse(normal),
            insert: parse(insert),
        }
    }
}

pub enum Mode {
    Normal,
    Insert,
    Cmdline,
}

impl Keymap {
    pub fn maybe_with_defaults(self) -> Self {
        if self.clear_defaults {
            return self;
        }

        let mut keymap = Keymap::default();
        keymap.cmdline.extend(self.cmdline);
        keymap.normal.extend(self.normal);
        keymap.insert.extend(self.insert);
        keymap
    }

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

    #[test]
    fn parses_command_with_optional_arg() {
        assert_eq!(
            "tab_select 2".parse::<Command>().unwrap(),
            Command {
                name: CommandName::TabSelect,
                args: Some("2".into()),
            }
        );
        assert_eq!(
            "assistant_prev".parse::<Command>().unwrap(),
            Command {
                name: CommandName::AssistantPrev,
                args: None,
            }
        );
    }
}

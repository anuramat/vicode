use std::str::FromStr;

use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use schemars::JsonSchema;
use schemars::json_schema;
use serde_plain::derive_deserialize_from_fromstr;
use serde_plain::derive_serialize_from_display;

derive_deserialize_from_fromstr!(KeyChord, "valid key chord");
derive_serialize_from_display!(KeyChord);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl JsonSchema for KeyChord {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "KeyChord".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
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
            "enter" | "cr" | "return" => KeyCode::Enter,
            "esc" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" | "bs" => KeyCode::Backspace,
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

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

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
}

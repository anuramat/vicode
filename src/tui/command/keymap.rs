use std::collections::HashMap;
use std::mem;
use std::sync::LazyLock;

use crossterm::event::KeyEvent;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use super::Command;
use super::CommandName;
use super::KeyChord;

// TODO allow multiple commands per chord?

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
#[serde(default)]
pub struct Keymap {
    /// if false, keymaps are merged with defaults
    pub clear_defaults: bool,
    pub cmdline: HashMap<KeyChord, Command>,
    pub normal: HashMap<KeyChord, Command>,
    pub insert: HashMap<KeyChord, Command>,
}

static DEFAULT_KEYMAP: LazyLock<Keymap> = LazyLock::new(parse_default_keymap);

fn parse_default_keymap() -> Keymap {
    toml::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/default/keymap.toml"
    )))
    .expect("default keymap must be valid")
}

impl Keymap {
    pub fn default_keymap() -> Self {
        DEFAULT_KEYMAP.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Cmdline,
}

impl Keymap {
    pub fn merge_default(&mut self) {
        if self.clear_defaults {
            return;
        }
        let user = mem::take(self);
        let mut merged = Self::default_keymap();
        merged.cmdline.extend(user.cmdline);
        merged.normal.extend(user.normal);
        merged.insert.extend(user.insert);
        *self = merged;
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
    use similar_asserts::assert_eq;

    use super::*;

    #[test]
    fn default_keymap_toml_parses() {
        let keymap = parse_default_keymap();
        let enter: KeyChord = "enter".parse().unwrap();
        assert_eq!(
            keymap.cmdline.get(&enter),
            Some(&Command {
                name: CommandName::InputSubmit,
                args: None
            })
        );
    }
}

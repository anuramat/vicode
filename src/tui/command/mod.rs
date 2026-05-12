use std::str::FromStr;

use anyhow::Result;
use schemars::JsonSchema;
use serde_plain::derive_deserialize_from_fromstr;
use serde_plain::derive_serialize_from_display;

pub mod chord;
pub mod keymap;
pub mod name;

pub use chord::KeyChord;
pub use keymap::Keymap;
pub use name::CommandName;

pub fn parse_arg<T>(args: Option<&str>) -> Result<Option<T>>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    args.map(|s| {
        s.parse::<T>()
            .map_err(|e| anyhow::anyhow!("invalid argument: {s} ({e})"))
    })
    .transpose()
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
        Ok(Self {
            name: serde_plain::from_str(name)?,
            args,
        })
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;

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

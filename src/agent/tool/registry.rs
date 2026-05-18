use derive_more::Deref;
use derive_more::DerefMut;
use derive_more::From;
use derive_more::Into;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

inventory::collect!(ToolDeclaration);

#[derive(Clone, Debug)]
pub struct ToolDeclaration(pub fn() -> ToolSchema);

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, From, Into, Deref, DerefMut)]
pub struct ToolSchemas(pub Vec<ToolSchema>);

impl Default for ToolSchemas {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSchemas {
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn new() -> Self {
        Self(
            inventory::iter::<ToolDeclaration>
                .into_iter()
                .map(|declaration| declaration.0())
                .collect(),
        )
    }
}

#[macro_export]
macro_rules! declare_tool {
    (name: $name:literal, description: $description:expr, call: $call:ident, arguments: $arguments:ty, meta: $meta:ty, result: $result:ty $(,)?) => {
        #[allow(dead_code)]
        pub const TOOL_NAME: &str = $name;

        pub type $call = $crate::agent::tool::generic::GenericTask<$arguments, $meta, $result>;

        #[typetag::serde(name = $name)]
        impl $crate::agent::tool::traits::ToolCallSerializable for $call {}

        fn declaration() -> $crate::agent::tool::registry::ToolSchema {
            $crate::agent::tool::registry::ToolSchema {
                name: $name.to_string(),
                description: $description.to_string(),
                parameters: schemars::schema_for!($arguments).to_value(),
            }
        }

        inventory::submit! {
            $crate::agent::tool::registry::ToolDeclaration(
                declaration as fn() -> $crate::agent::tool::registry::ToolSchema
            )
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toolkit_schema() {
        let schema = ToolSchemas::new();

        let bash_schema = schema
            .0
            .iter()
            .find(|schema| schema.name == "bash")
            .expect("bash tool not found");

        let serialized = serde_json::to_value(bash_schema).unwrap();
        insta::assert_json_snapshot!(serialized, @r#"
        {
          "description": "Execute a bash command in a sandboxed environment.",
          "name": "bash",
          "parameters": {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "additionalProperties": false,
            "properties": {
              "command": {
                "description": "The bash command to execute.",
                "type": "string"
              }
            },
            "required": [
              "command"
            ],
            "title": "BashArguments",
            "type": "object"
          }
        }
        "#);
    }
}

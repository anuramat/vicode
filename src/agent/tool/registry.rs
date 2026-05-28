use std::sync::LazyLock;

use derive_getters::Getters;
use derive_more::Deref;
use derive_more::DerefMut;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::llm::history::TokenCount;
use crate::llm::history::count_text_tokens;

inventory::collect!(ToolDeclaration);

pub static TOOL_REGISTRY: LazyLock<ToolRegistry> = LazyLock::new(ToolRegistry::from_inventory);

#[derive(Clone, Debug)]
pub struct ToolDeclaration(pub fn() -> ToolSchema);

#[derive(Clone, Serialize, Deserialize, Debug, Getters)]
pub struct ToolSchema {
    name: String,
    description: String,
    parameters: Value,
    // skipping so we don't send it to the provider
    #[serde(skip)]
    #[getter(skip)]
    token_count: usize,
}

#[derive(Clone, Debug, Deref, DerefMut)]
pub struct ToolRegistry {
    #[deref]
    #[deref_mut]
    pub schemas: Vec<ToolSchema>,
    pub token_count: usize,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        TOOL_REGISTRY.clone()
    }
}

impl ToolRegistry {
    pub fn empty() -> Self {
        Vec::new().into()
    }

    pub fn without<I, S>(
        &self,
        names: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: ToString,
    {
        let names = names
            .into_iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let mut result: Self = self
            .iter()
            .filter(|tool| !names.iter().any(|name| name == tool.name()))
            .cloned()
            .collect::<Vec<_>>()
            .into();
        result.recount();
        result
    }

    fn from_inventory() -> Self {
        inventory::iter::<ToolDeclaration>
            .into_iter()
            .map(|declaration| declaration.0())
            .collect::<Vec<_>>()
            .into()
    }
}

impl ToolSchema {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        let mut result = Self {
            name: name.into(),
            description: description.into(),
            parameters,
            token_count: 0,
        };
        result.recount();
        result
    }
}

impl From<Vec<ToolSchema>> for ToolRegistry {
    fn from(schemas: Vec<ToolSchema>) -> Self {
        let mut result = Self {
            schemas,
            token_count: 0,
        };
        result.recount();
        result
    }
}

impl TokenCount for ToolRegistry {
    fn token_count(&self) -> usize {
        self.token_count
    }

    fn recount(&mut self) {
        self.schemas.iter_mut().for_each(ToolSchema::recount);
        self.token_count = self.schemas.iter().map(ToolSchema::token_count).sum();
    }
}

impl TokenCount for ToolSchema {
    fn token_count(&self) -> usize {
        self.token_count
    }

    fn recount(&mut self) {
        let text = serde_json::to_string(self).expect("could not serialize tool schema");
        self.token_count = count_text_tokens(&text);
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
            $crate::agent::tool::registry::ToolSchema::new(
                $name,
                $description,
                schemars::schema_for!($arguments).to_value(),
            )
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
        let registry = TOOL_REGISTRY.clone();

        let bash_schema = registry
            .iter()
            .find(|schema| schema.name() == "bash")
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

    #[test]
    fn tool_registry_token_count_is_cached_and_nonzero() {
        use similar_asserts::assert_eq;

        assert!(TOOL_REGISTRY.token_count() > 0);
        assert_eq!(
            TOOL_REGISTRY.token_count(),
            ToolRegistry::default().token_count()
        );
        assert_eq!(ToolRegistry::empty().token_count(), 0);
    }

    #[test]
    fn without_excludes_named_tools() {
        use similar_asserts::assert_eq;

        let excluded = ["bash"];
        let registry = TOOL_REGISTRY.without(&excluded);

        assert!(!registry.iter().any(|tool| tool.name() == "bash"));
        assert_eq!(registry.len(), TOOL_REGISTRY.len() - 1);
        assert!(registry.token_count() < TOOL_REGISTRY.token_count());
    }
}

pub mod task;
pub mod widget;

use crate::declare_tool;
use crate::sandbox::SandboxRunner;

declare_tool! {
    name: "bash",
    description: "Execute a bash command in a sandboxed environment.",
    call: BashCall,
    arguments: BashArguments,
    context: BashContext,
    meta: (),
    result: BashResult,
}

#[derive(
    Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct BashArguments {
    #[schemars(description = "The bash command to execute.")]
    pub command: String,
}

#[derive(Clone, Debug)]
pub struct BashContext {
    runner: SandboxRunner,
    shell_cmd: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BashResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<i32>,
    pub signal: Option<i32>,
}

// test schema generation

#[cfg(test)]
mod tests {
    use schemars::schema_for;
    use serde_json::json;
    use similar_asserts::assert_eq;

    use super::*;

    #[test]
    fn test_bash_arguments_schema() {
        let expected_schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "BashArguments",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute."
                }
            },
            "required": ["command"]
        });

        let generated_schema = schema_for!(BashArguments).to_value();

        assert_eq!(generated_schema, expected_schema);
    }
}

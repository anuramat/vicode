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
    Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

    use super::*;

    #[test]
    fn test_bash_arguments_schema() {
        let generated_schema = schema_for!(BashArguments).to_value();
        insta::assert_json_snapshot!(generated_schema, @r#"
        {
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
        "#);
    }
}

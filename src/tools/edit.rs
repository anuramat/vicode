use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use serde::Deserialize;
use serde::Serialize;
use similar::TextDiff;

use crate::agent::tool::traits::*;
use crate::declare_tool;
use crate::project::PROJECT;
use crate::project::layout::LayoutTrait;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::message::toolcall::ToolCallWidget;
use crate::tui::widgets::syntax::HIGHLIGHTER;

#[derive(
    Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct EditArguments {
    #[schemars(
        description = "Path to the file to edit. Can be absolute or relative to the workdir."
    )]
    pub filepath: String,
    #[schemars(description = "Sequence of edits to perform.")]
    pub edits: Vec<Edit>,
}

#[derive(
    Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    #[schemars(
        description = "Exact string in the file to be replaced; if empty, the replacement will overwrite the entire file, creating it if it doesn't exist."
    )]
    pub pattern: String,
    #[schemars(description = "String to replace the pattern with.")]
    pub replacement: String,
    #[schemars(description = "Whether to replace all occurrences of the pattern.")]
    pub replace_all: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EditResult {
    pub success: bool,
}

declare_tool!(
    name: "edit",
    description: "Edit a file by applying one or more string replacements.",
    call: EditCall,
    arguments: EditArguments,
    context: EditContext,
    meta: EditMeta,
    result: EditResult,
);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditMeta {
    diff: String,
}

#[derive(Debug, Clone)]
pub struct EditContext {
    workdir: PathBuf,
}

impl ToolContext<EditArguments> for EditContext {
    fn prepare(
        _: &EditArguments,
        agent: &crate::agent::Agent,
    ) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(Self {
            workdir: PROJECT.agent_workdir(&agent.id),
        })
    }
}

#[async_trait::async_trait]
impl Function<EditContext, EditMeta, EditResult> for EditArguments {
    async fn call(
        &self,
        ctx: EditContext,
    ) -> Result<(EditResult, EditMeta)> {
        let target_path = {
            let path = Path::new(&self.filepath);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                ctx.workdir.join(path)
            }
        };
        let diff = edit_file(&target_path, &self.edits)?;
        Ok((EditResult { success: true }, EditMeta { diff }))
    }
}

impl From<&EditCall> for Element {
    fn from(call: &EditCall) -> Element {
        let text: Option<Text<'_>> = if let Some(meta) = &call.meta {
            Some(HIGHLIGHTER.highlight(&meta.diff, &HIGHLIGHTER.diff))
        } else {
            call.output().map(|o| o.into())
        };
        ToolCallWidget {
            name: format!("edit: {}", call.arguments.clone().unwrap().filepath),
            inner: text.map(Paragraph::new),
        }
        .into()
    }
}

fn edit_file(
    target_path: &Path,
    edits: &[Edit],
) -> Result<String> {
    let (contents, new_contents) = apply_edits(target_path, edits)?;
    fs::write(target_path, &new_contents)?;
    Ok(TextDiff::from_lines(&contents, &new_contents)
        .unified_diff()
        .to_string())
}

fn apply_edits(
    target_path: &Path,
    edits: &[Edit],
) -> Result<(String, String)> {
    let original = read(target_path)?;
    let mut result = original.clone();

    for (i, edit) in edits.iter().enumerate() {
        if edit.pattern.is_empty() {
            result = edit.replacement.clone();
        } else {
            result = replace(result, &edit.pattern, &edit.replacement, edit.replace_all)
                .with_context(|| format!("failed to apply edit {}/{}", i + 1, edits.len()))?;
        }
    }

    Ok((original, result))
}

fn read(path: &Path) -> Result<String> {
    // TODO show to user if a new file was created?
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err.into()),
    }
}

fn replace(
    text: String,
    pattern: &str,
    replacement: &str,
    replace_all: bool,
) -> Result<String> {
    if !text.contains(pattern) {
        return Err(anyhow::anyhow!("no match found"));
    }
    Ok(if replace_all {
        text.replace(pattern, replacement)
    } else {
        let result = text.replacen(pattern, replacement, 1);
        if result.contains(pattern) {
            return Err(anyhow::anyhow!(
                "multiple matches found when replace_all is false"
            ));
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    use serde_json::json;

    use super::*;

    #[test]
    fn edit_arguments_deserialize_new_schema() {
        let args: EditArguments = serde_json::from_value(json!({
            "filepath": "foo.rs",
            "edits": [
                {"pattern": "a", "replacement": "b", "replace_all": false},
                {"pattern": "b", "replacement": "c", "replace_all": true}
            ]
        }))
        .unwrap();

        assert_eq!(args.filepath, "foo.rs");
        assert_eq!(args.edits.len(), 2);
        assert!(args.edits[1].replace_all);
    }

    #[test]
    fn replace_all_matches_replaces_every_match() {
        assert_eq!(
            replace(String::from("a b a"), "a", "x", true).unwrap(),
            "x b x"
        );
    }

    #[test]
    fn applies_edits_sequentially() {
        let dir = temp_dir();
        let path = dir.join("file.txt");
        fs::write(&path, "hello world").unwrap();

        edit_file(
            &path,
            &[
                Edit {
                    pattern: "hello".into(),
                    replacement: "hi".into(),
                    replace_all: false,
                },
                Edit {
                    pattern: "hi world".into(),
                    replacement: "done".into(),
                    replace_all: false,
                },
            ],
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "done");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn empty_pattern_overwrites_existing_file() {
        let dir = temp_dir();
        let path = dir.join("file.txt");
        fs::write(&path, "old").unwrap();

        edit_file(
            &path,
            &[Edit {
                pattern: String::new(),
                replacement: "new".into(),
                replace_all: false,
            }],
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn empty_pattern_creates_missing_file() {
        let dir = temp_dir();
        let path = dir.join("file.txt");

        edit_file(
            &path,
            &[Edit {
                pattern: String::new(),
                replacement: "new".into(),
                replace_all: false,
            }],
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        fs::remove_dir_all(dir).unwrap();
    }

    fn temp_dir() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "vicode-edit-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        path
    }
}

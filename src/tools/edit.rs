use std::fs;
use std::path::Path;
use std::path::PathBuf;

use ansi_to_tui::IntoText;
use anyhow::Result;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use serde::Deserialize;
use serde::Serialize;
use similar::TextDiff;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use syntect::util::as_24_bit_terminal_escaped;

use crate::agent::tool::traits::*;
use crate::declare_tool;
use crate::project::PROJECT;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::container::element::IntoElement;
use crate::tui::widgets::message::toolcall::ToolCallWidget;

// TODO add option to create a new file/replace existing

#[derive(
    Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct EditArguments {
    #[schemars(
        description = "Path to the file to edit. Can be absolute or relative to the workdir."
    )]
    pub filepath: String,
    #[schemars(description = "Exact string in the file to be replaced.")]
    pub pattern: String,
    #[schemars(description = "String to replace the pattern with.")]
    pub replacement: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EditResult {
    pub success: bool,
}

declare_tool!(
    name: "edit",
    description: "Edit a file by replacing a single occurrence of a string.",
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

        let contents = fs::read_to_string(&target_path)?;
        let pattern = &self.pattern;
        let replacement = &self.replacement;
        let new_contents = replace_one(&contents, pattern, replacement)?;
        // TODO atomic write (write to temp and move)
        fs::write(&target_path, &new_contents)?;
        let diff = TextDiff::from_lines(&contents, &new_contents)
            .unified_diff()
            .to_string();
        Ok((EditResult { success: true }, EditMeta { diff }))
    }
}

static SYNTAX_SET: std::sync::LazyLock<SyntaxSet> =
    std::sync::LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: std::sync::LazyLock<ThemeSet> = std::sync::LazyLock::new(ThemeSet::load_defaults);

impl From<&EditCall> for Element {
    fn from(call: &EditCall) -> Element {
        let text: Text<'_> = if let Some(meta) = &call.meta {
            // TODO optimize, parameterize theme, unify with bash widget rendering thing
            let syntax = SYNTAX_SET.find_syntax_by_token("diff").unwrap();
            let theme = &THEME_SET.themes["base16-ocean.dark"];
            let mut highlighter = HighlightLines::new(syntax, theme);

            LinesWithEndings::from(&meta.diff)
                .filter_map(|line| highlighter.highlight_line(line, &SYNTAX_SET).ok())
                .filter_map(|parts| as_24_bit_terminal_escaped(&parts, false).into_text().ok())
                .flatten()
                .collect()
        } else if let Some(output) = call.output() {
            output.into()
        } else {
            "pending".into()
        };
        ToolCallWidget {
            name: format!("edit: {}", call.arguments.clone().unwrap().filepath),
            inner: Paragraph::new(text),
        }
        .into()
    }
}

pub fn replace_one(
    contents: &str,
    old: &str,
    new: &str,
) -> Result<String> {
    if old.is_empty() {
        return Err(anyhow::anyhow!("pattern must not be empty"));
    }

    let mut matches = contents.match_indices(old);
    let start = match matches.next() {
        Some((start, _)) => start,
        None => return Err(anyhow::anyhow!("no match found")),
    };
    if matches.next().is_some() {
        return Err(anyhow::anyhow!("multiple matches found"));
    }
    let end = start + old.len();

    let mut result = String::with_capacity(contents.len() - old.len() + new.len());
    result.push_str(&contents[..start]);
    result.push_str(new);
    result.push_str(&contents[end..]);

    Ok(result)
}

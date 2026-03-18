use anyhow::Result;
use ratatui::prelude::*;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use crate::agent::tool::traits::*;
use crate::declare_tool;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::message::toolcall::ToolCallWidget;

declare_tool!(
    name: "todo",
    description: "Manages a todo list. Keeps track of tasks that are pending, in progress, or completed. Useful for organizing and prioritizing tasks.",
    call: TodoCall,
    arguments: TodoArguments,
    context: (),
    meta: (),
    result: TodoResult,
);

#[derive(
    Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct TodoArguments {
    #[serde(flatten)]
    pub state: TodoState,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TodoResult {}

#[derive(
    Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct TodoState {
    #[schemars(description = "Description of the task assistant is currently working on.")]
    pub current: String,
    pub entries: Vec<TodoEntry>,
}

#[derive(
    Clone, Default, Debug, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub enum EntryStatus {
    #[default]
    Pending,
    InProgress,
    Done,
}

#[derive(
    Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct TodoEntry {
    pub task: String,
    pub status: EntryStatus,
}

#[async_trait::async_trait]
impl Function<(), (), TodoResult> for TodoArguments {
    async fn call(
        &self,
        _: (),
    ) -> Result<(TodoResult, ())> {
        Ok((TodoResult {}, ()))
    }
}

lazy_static::lazy_static! {
    static ref BLOCK: ratatui::widgets::Block<'static> = ratatui::widgets::Block::bordered().border_set(ratatui::symbols::border::PLAIN).title("");
}

lazy_static::lazy_static! {
    static ref STYLE: Style = Style::default().italic();
}

impl From<&TodoCall> for Element {
    fn from(call: &TodoCall) -> Self {
        let mut name = "todo updated".to_string();
        let inner = call.arguments.as_ref().map(|args| {
            name = format!("todo: {}", args.state.current);
            let mut lines = Vec::new();
            for entry in &args.state.entries {
                let marker = match entry.status {
                    EntryStatus::Done => "[x]",
                    EntryStatus::InProgress => "[~]",
                    EntryStatus::Pending => "[ ]",
                };
                lines.push(format!("{} {}", marker, entry.task));
            }
            Paragraph::new(lines.join("\n")).wrap(Wrap { trim: false })
        });

        ToolCallWidget { name, inner }.into()
    }
}

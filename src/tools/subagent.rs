use std::fmt::Write;

use anyhow::Result;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::agent::Agent;
use crate::agent::subagent;
use crate::agent::subagent::SubagentHandle;
use crate::agent::subagent::SubagentResult;
use crate::agent::tool::traits::Function;
use crate::agent::tool::traits::ToolContext;
use crate::declare_tool;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::message::toolcall::ToolCallWidget;
use crate::tui::widgets::syntax::HIGHLIGHTER;

declare_tool!(
    name: "subagent",
    description: "Run a child agent in its own workdir and return its response plus the diff it produced.",
    call: SubagentCall,
    arguments: SubagentArguments,
    context: SubagentContext,
    meta: (),
    result: SubagentResult,
);

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubagentArguments {
    #[schemars(description = "The prompt sent to the subagent.")]
    pub prompt: String,
    #[schemars(
        description = "Whether the subagent should inherit the context of the parent agent. If false, the subagent will start with an empty context."
    )]
    pub inherit_context: bool,
}

#[derive(Debug)]
pub struct SubagentContext {
    handle: SubagentHandle,
}

#[async_trait::async_trait]
impl ToolContext<SubagentArguments> for SubagentContext {
    async fn prepare(
        args: &SubagentArguments,
        agent: &Agent,
    ) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(Self {
            handle: subagent::spawn(agent, args.prompt.clone(), args.inherit_context).await?,
        })
    }
}

#[async_trait::async_trait]
impl Function<SubagentContext, (), SubagentResult> for SubagentArguments {
    async fn call(
        &self,
        ctx: SubagentContext,
    ) -> Result<(SubagentResult, ())> {
        let result = ctx.handle.wait().await?;
        Ok((
            SubagentResult {
                output: result.output,
                diff: result.diff,
            },
            (),
        ))
    }
}

impl From<&SubagentCall> for Element {
    fn from(call: &SubagentCall) -> Self {
        let mut name = "subagent: ".to_string();
        let inner = call.output.as_ref().map(|output| {
            match output {
                Ok(result) => {
                    write!(
                        name,
                        "{} chars output, {} chars diff",
                        result.output.len(),
                        result.diff.len()
                    )
                    .unwrap();
                    Paragraph::new(text(result))
                }
                Err(err) => {
                    write!(name, "error: {err}").unwrap();
                    Paragraph::new(err.clone())
                }
            }
            .wrap(Wrap { trim: false })
        });
        ToolCallWidget { name, inner }.into()
    }
}

fn text(result: &SubagentResult) -> Text<'static> {
    let mut text = HIGHLIGHTER.highlight(&result.output, &HIGHLIGHTER.markdown);
    if !result.diff.is_empty() {
        text.extend(Text::from("\n\n"));
        text.extend(HIGHLIGHTER.highlight(&result.diff, &HIGHLIGHTER.diff));
    }
    text
}

use anyhow::Context;
use anyhow::Result;
use ratatui::widgets::Paragraph;

use crate::agent::router::api::ListEntry;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::agent::tool::traits::Function;
use crate::agent::tool::traits::ToolCall;
use crate::declare_tool;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::message::toolcall::ToolCallWidget;

declare_tool!(
    name: "list",
    description: "Enumerate your tab's members — {id, parent, status} — or, with subtree, \
        only your own spawn-descendants. The id-recovery path when an agent's id has \
        fallen out of your history.",
    call: ListCall,
    arguments: ListArguments,
    meta: (),
    result: Vec<ListEntry>,
);

#[derive(
    Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct ListArguments {
    #[schemars(description = "Restrict the listing to your own spawn-descendants.")]
    pub subtree: bool,
}

#[async_trait::async_trait]
impl Function<(), Vec<ListEntry>> for ListArguments {
    async fn call(
        &self,
        ctx: ToolRuntimeContext,
    ) -> Result<(Vec<ListEntry>, ())> {
        let members = ctx
            .router
            .list(ctx.agent_id, self.subtree)
            .await?
            .context("unknown caller")?;
        Ok((members, ()))
    }
}

impl From<&ListCall> for Element {
    fn from(call: &ListCall) -> Self {
        ToolCallWidget {
            name: "list".into(),
            inner: call.output().map(Paragraph::new),
        }
        .into()
    }
}

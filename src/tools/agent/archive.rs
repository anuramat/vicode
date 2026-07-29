use anyhow::Result;
use ratatui::widgets::Paragraph;

use crate::agent::id::AgentId;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::agent::tool::traits::Function;
use crate::agent::tool::traits::ToolCall;
use crate::declare_tool;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::message::toolcall::ToolCallWidget;

declare_tool!(
    name: "archive",
    description: "Tear down an agent you spawned — a strict spawn-descendant — and its whole \
        subtree: unreachable from then on, runtime and mount released, history and workdir \
        retained. Expected hygiene once you've collected an agent's result; archived agents \
        stop counting against your tab.",
    call: ArchiveCall,
    arguments: ArchiveArguments,
    meta: (),
    result: (),
);

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct ArchiveArguments {
    #[schemars(description = "The target agent's id.")]
    pub id: AgentId,
}

#[async_trait::async_trait]
impl Function<(), ()> for ArchiveArguments {
    async fn call(
        &self,
        ctx: ToolRuntimeContext,
    ) -> Result<((), ())> {
        ctx.router.archive(ctx.agent_id, self.id.clone()).await??;
        Ok(((), ()))
    }
}

impl From<&ArchiveCall> for Element {
    fn from(call: &ArchiveCall) -> Self {
        ToolCallWidget {
            name: call
                .arguments
                .as_ref()
                .map_or_else(|| "archive".into(), |a| format!("archive: {}", a.id)),
            inner: call.output().map(Paragraph::new),
        }
        .into()
    }
}

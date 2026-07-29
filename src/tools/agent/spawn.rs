use anyhow::Context;
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
    name: "spawn",
    description: "Spawn a new agent in an isolated copy of your working directory. It starts \
        on the prompt immediately and works asynchronously; the returned id is its address \
        for send/inspect/wait/archive. Collect its result with wait (finishing its turn IS \
        its report), its file changes with inspect.",
    call: SpawnCall,
    arguments: SpawnArguments,
    meta: (),
    result: SpawnResult,
);

#[derive(
    Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct SpawnArguments {
    #[schemars(description = "The task prompt, delivered as the new agent's opening message.")]
    pub prompt: String,
    #[schemars(description = "Whether the new agent sees your conversation so far.")]
    pub inherit_context: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpawnResult {
    pub id: AgentId,
}

#[async_trait::async_trait]
impl Function<(), SpawnResult> for SpawnArguments {
    async fn call(
        &self,
        ctx: ToolRuntimeContext,
    ) -> Result<(SpawnResult, ())> {
        let capture = ctx
            .capture
            .context("spawn dispatched without a history capture")?;
        let id = ctx
            .router
            .spawn_agent(ctx.agent_id, capture, self.prompt.clone())
            .await?;
        Ok((SpawnResult { id }, ()))
    }

    fn spawn_capture(&self) -> Option<bool> {
        Some(self.inherit_context)
    }
}

impl From<&SpawnCall> for Element {
    fn from(call: &SpawnCall) -> Self {
        ToolCallWidget {
            name: call
                .arguments
                .as_ref()
                .map_or_else(|| "spawn".into(), |a| format!("spawn: {}", a.prompt)),
            inner: call.output().map(Paragraph::new),
        }
        .into()
    }
}

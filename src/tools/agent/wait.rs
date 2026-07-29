use anyhow::Result;
use ratatui::widgets::Paragraph;

use crate::agent::id::AgentId;
use crate::agent::router::api::WaitResult;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::agent::tool::traits::Function;
use crate::agent::tool::traits::ToolCall;
use crate::declare_tool;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::message::toolcall::ToolCallWidget;

declare_tool!(
    name: "wait",
    description: "Suspend until the target agent next goes idle (or dies), then return how \
        it fired plus its last output text — the way to collect a spawned agent's result. \
        Fires immediately if the target is already idle; a freshly-messaged target counts \
        as woken, so a follow-up wait collects the reply. A death or a failed turn is typed \
        (`status`/`error`, the last good output preserved), never returned as the \
        answer; a wait that would close a wait cycle errors instead of registering.",
    call: WaitCall,
    arguments: WaitArguments,
    meta: (),
    result: WaitResult,
);

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct WaitArguments {
    #[schemars(description = "The target agent's id.")]
    pub id: AgentId,
}

#[async_trait::async_trait]
impl Function<(), WaitResult> for WaitArguments {
    async fn call(
        &self,
        ctx: ToolRuntimeContext,
    ) -> Result<(WaitResult, ())> {
        Ok((ctx.router.wait(ctx.agent_id, self.id.clone()).await??, ()))
    }
}

impl From<&WaitCall> for Element {
    fn from(call: &WaitCall) -> Self {
        ToolCallWidget {
            name: call
                .arguments
                .as_ref()
                .map_or_else(|| "wait".into(), |a| format!("wait: {}", a.id)),
            inner: call.output().map(Paragraph::new),
        }
        .into()
    }
}

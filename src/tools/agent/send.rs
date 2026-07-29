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
    name: "send",
    description: "Send a plain-text message to another agent in your tab. Non-blocking: \
        success means accepted into its mailbox (wakes an idle target, or is buffered until \
        its current turn ends) — not durably delivered; a full mailbox errors, retry later. \
        Use it for mid-work updates and questions; collect a finished agent's result with \
        wait, not send.",
    call: SendCall,
    arguments: SendArguments,
    meta: (),
    result: (),
);

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct SendArguments {
    #[schemars(description = "The target agent's id.")]
    pub id: AgentId,
    #[schemars(description = "The message text.")]
    pub text: String,
}

#[async_trait::async_trait]
impl Function<(), ()> for SendArguments {
    async fn call(
        &self,
        ctx: ToolRuntimeContext,
    ) -> Result<((), ())> {
        ctx.router
            .send_message(ctx.agent_id, self.id.clone(), self.text.clone())
            .await??;
        Ok(((), ()))
    }
}

impl From<&SendCall> for Element {
    fn from(call: &SendCall) -> Self {
        ToolCallWidget {
            name: call
                .arguments
                .as_ref()
                .map_or_else(|| "send".into(), |a| format!("send: {}", a.id)),
            inner: call.output().map(Paragraph::new),
        }
        .into()
    }
}

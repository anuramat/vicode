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
    description: "Spawn a new agent on its own git branch `vc-<id>`, checked out at `commit` \
        (default: your HEAD) -- your uncommitted changes are not included, so commit what it \
        needs first. It starts on the prompt immediately and works asynchronously; the \
        returned id is its address for send/archive. It reports back with send: to wait for \
        its report, end your turn -- the message wakes you. Its committed work is on its \
        branch (`git log <commit>..vc-<id>`, `git diff <commit> vc-<id>`), ready to merge or \
        cherry-pick.",
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
    #[schemars(
        description = "The id (full or abbreviated) of the commit the new agent starts from; \
            defaults to your HEAD."
    )]
    pub commit: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpawnResult {
    pub id: AgentId,
    /// the resolved start commit
    pub commit: String,
}

#[async_trait::async_trait]
impl Function<(), SpawnResult> for SpawnArguments {
    async fn call(
        &self,
        ctx: ToolRuntimeContext,
    ) -> Result<(SpawnResult, ())> {
        let commit = crate::git::resolve(&ctx.workdir(), self.commit.as_deref())?;
        let id = ctx
            .router
            .spawn_agent(&ctx.agent_id, &commit, ctx.inherited_history, &self.prompt)
            .await?;
        Ok((SpawnResult { id, commit }, ()))
    }

    fn inherit_history(&self) -> bool {
        self.inherit_context
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

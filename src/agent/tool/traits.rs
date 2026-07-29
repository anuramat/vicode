use std::fmt::Debug;

use anyhow::Result;
use dyn_clone::DynClone;

use crate::agent::tool::context::ToolRuntimeContext;
use crate::tui::widgets::container::element::IntoElement;

dyn_clone::clone_trait_object!(ToolCallSerializable);
#[typetag::serde(tag = "name")]
pub trait ToolCallSerializable: Debug + Send + Sync + DynClone + IntoElement + ToolCall {}

#[async_trait::async_trait]
pub trait ToolCall: Send + Sync {
    fn arguments(&self) -> String;
    /// None if the tool call wasn't executed yet
    fn output(&self) -> Option<String>;

    async fn run(
        &mut self,
        ctx: ToolRuntimeContext,
    );

    /// repair hook for abort/restore: fail the call iff it has no output yet
    fn fail_unresolved(
        &mut self,
        _msg: &str,
    ) {
    }

    /// §2.4a compose: the reaper hands a successful call the `Agent`'s
    /// accumulated stream; a streaming tool adopts it as its authoritative
    /// output text, a non-streaming tool ignores it
    fn compose(
        &mut self,
        _streamed: String,
    ) {
    }

    /// `Some(inherit)` only for `spawn`: tells the core to snapshot its live
    /// history into the dispatch (§2.2)
    fn spawn_capture(&self) -> Option<bool> {
        None
    }
}

#[async_trait::async_trait]
pub trait Function<TMeta = (), TResult = ()>: Send + Sync {
    async fn call(
        &self,
        ctx: ToolRuntimeContext,
    ) -> Result<(TResult, TMeta)>;

    /// see [`ToolCall::spawn_capture`]
    fn spawn_capture(&self) -> Option<bool> {
        None
    }

    /// see [`ToolCall::compose`]
    fn compose(
        _result: &mut TResult,
        _streamed: String,
    ) {
    }
}

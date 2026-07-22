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

    /// should be no-op if output exists
    fn fail_unresolved(
        &mut self,
        msg: &str,
    );

    fn compose(
        &mut self,
        _streamed: String,
    ) {
    }

    fn inherit_history(&self) -> bool {
        false
    }
}

#[async_trait::async_trait]
pub trait Function<TMeta = (), TResult = ()>: Send + Sync {
    async fn call(
        &self,
        ctx: ToolRuntimeContext,
    ) -> Result<(TResult, TMeta)>;

    fn inherit_history(&self) -> bool {
        false
    }

    fn compose(
        _result: &mut TResult,
        _streamed: String,
    ) {
    }
}

use std::fmt::Debug;

use anyhow::Result;
use dyn_clone::DynClone;

use crate::agent::Agent;
use crate::tui::widgets::container::element::IntoElement;

dyn_clone::clone_trait_object!(ToolCallSerializable);
#[typetag::serde(tag = "name")]
pub trait ToolCallSerializable: Debug + Send + Sync + DynClone + IntoElement + ToolCall {}

#[async_trait::async_trait]
pub trait ToolCall: Send + Sync {
    fn arguments(&self) -> String;
    /// None if the tool call wasn't executed yet
    fn output(&self) -> Option<String>;

    async fn run(&mut self);

    async fn prepare(
        &mut self,
        _agent: &Agent,
    ) -> Result<()>;
}

// some helper traits:

#[async_trait::async_trait]
pub trait ToolContext<TArgs>: Send + Sync {
    async fn prepare(
        _args: &TArgs,
        _agent: &Agent,
    ) -> Result<Self>
    where
        Self: Sized;
}

#[async_trait::async_trait]
pub trait Function<TCtx = (), TMeta = (), TResult = ()>: Send + Sync {
    async fn call(
        &self,
        ctx: TCtx,
    ) -> Result<(TResult, TMeta)>;
}

#[async_trait::async_trait]
impl<TArgs> ToolContext<TArgs> for () {
    async fn prepare(
        _args: &TArgs,
        _agent: &Agent,
    ) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(())
    }
}

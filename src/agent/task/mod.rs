pub mod handle;
pub mod manager;
pub mod sink;

use anyhow::Result;
use manager::TaskId;

use crate::agent::Agent;

#[async_trait::async_trait]
pub trait TaskDelta: Send + Sync + std::fmt::Debug {
    async fn apply(
        self: Box<Self>,
        agent: &mut Agent,
    ) -> Result<()>;
}

#[async_trait::async_trait]
pub trait TaskResult: Send + Sync + std::fmt::Debug {
    async fn apply(
        self: Box<Self>,
        agent: &mut Agent,
    ) -> Result<()>;
}

#[async_trait::async_trait]
impl TaskResult for () {
    async fn apply(
        self: Box<Self>,
        _: &mut Agent,
    ) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
pub enum TaskEvent {
    Delta(Box<dyn TaskDelta>),
    Result(Box<dyn TaskResult>),
    Error(String),
}

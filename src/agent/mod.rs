pub mod handle;
pub mod id;
pub mod init;
pub mod replica;
pub mod run;
pub mod subagent;
pub mod task;
pub mod tool;
pub mod turn;

use std::sync::Arc;

use anyhow::Result;
use futures::future::AbortHandle;
pub use id::*;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;

use crate::agent::handle::AgentEvent;
use crate::agent::handle::ExternalEvent;
use crate::agent::handle::ParentHandle;
use crate::agent::task::manager::AgentTaskManager;
use crate::agent::tool::registry::ToolSchemas;
use crate::llm::history::*;
use crate::llm::provider::assistant::Assistant;

#[derive(Debug, Clone)]
pub struct AgentHandle {
    pub tx: Sender<AgentEvent>,
    pub abort: AbortHandle, // TODO should we use tokio abort handle instead?
}

impl AgentHandle {
    pub async fn send(
        &self,
        event: ExternalEvent,
    ) -> Result<()> {
        self.tx.send(AgentEvent::External(event)).await?;
        Ok(())
    }
}

pub struct Agent {
    pub id: AgentId,
    /// serializable/persistent state
    pub state: AgentState,
    /// parent
    pub parent: ParentHandle,
    // agent event loop
    pub tx: Sender<AgentEvent>,
    pub rx: Receiver<AgentEvent>,
    /// manages jobs in the agent event loop
    pub tskmgr: AgentTaskManager,

    // meh
    pub assistant: Arc<Assistant>,
    pub tools: ToolSchemas,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct AgentState {
    pub topology: AgentTopology,
    pub context: AgentContext,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct AgentTopology {
    pub kind: AgentKind,
    pub children: Vec<AgentId>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct AgentContext {
    pub commit: String,
    pub history: History,
    pub instructions: String,
    pub assistant_id: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub enum AgentKind {
    #[default]
    Primary,
    Replica {
        parent: AgentId,
    },
    Subagent {
        parent: AgentId,
    },
}

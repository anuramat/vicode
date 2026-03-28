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

pub use handle::AgentEvent;
pub use id::*;
use serde::Deserialize;
use serde::Serialize;
use task::TaskId;
use task::TaskResult;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;

use crate::agent::handle::ParentHandle;
use crate::agent::task::AgentTaskManager;
use crate::agent::tool::registry::ToolSchemas;
use crate::llm::history::*;
use crate::llm::provider::assistant::Assistant;

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

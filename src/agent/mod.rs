pub mod handle;
pub mod init;
pub mod replica;
pub mod run;
pub mod subagent;
pub mod task;
pub mod tool;
pub mod turn;

use std::sync::Arc;

pub use handle::AgentEvent;
use serde::Deserialize;
use serde::Serialize;
use task::TaskId;
use task::TaskResult;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;

use crate::agent::handle::ParentEvent;
use crate::agent::task::AgentTaskManager;
use crate::agent::tool::registry::ToolSchemas;
use crate::llm::provider::assistant::Assistant;
use crate::llm::history::*;
use crate::new_id;

new_id!(AgentId);

pub struct Agent {
    pub id: AgentId,
    /// serializable/persistent state
    pub state: AgentState,
    /// parent
    pub parent: Sender<ParentEvent>,
    // agent event loop
    pub tx: Sender<AgentEvent>,
    pub rx: Receiver<AgentEvent>,
    /// manages jobs in the agent event loop
    pub tskmgr: AgentTaskManager,

    // meh
    pub assistant: Arc<Assistant>,
    pub tools: ToolSchemas,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentState {
    pub topology: AgentTopology,
    pub context: AgentContext,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentTopology {
    pub kind: AgentKind,
    pub children: Vec<AgentId>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentContext {
    pub commit: String,
    pub history: History,
    pub instructions: String,
    pub assistant_id: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum AgentKind {
    Primary,
    Replica { parent: AgentId },
    Subagent { parent: AgentId },
}

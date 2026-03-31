pub mod compact;
pub mod handle;
pub mod id;
pub mod init;
pub mod replica;
pub mod run;
pub mod subagent;
pub mod task;
pub mod tool;
pub mod turn;

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
use crate::llm::message::AssistantMessageStatus;
use crate::llm::message::Message;
use crate::llm::provider::assistant::Assistant;

#[derive(Debug, Clone)]
pub struct AgentHandle {
    pub tx: Sender<AgentEvent>,
    pub state: AgentState,
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
    /// persistent and/or visible in UI
    pub state: AgentState,
    /// parent
    pub parent: ParentHandle,
    // agent event loop
    pub tx: Sender<AgentEvent>,
    pub rx: Receiver<AgentEvent>,
    /// manages jobs in the agent event loop
    pub tskmgr: AgentTaskManager,
    pub tools: ToolSchemas,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentState {
    #[serde(skip)]
    pub status: AgentStatus,
    pub assistant: Assistant,
    pub topology: AgentTopology,
    pub context: AgentContext,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
pub enum AgentStatus {
    Compacting,
    InProgress,
    #[default]
    Idle, // TODO maybe rename this, since Error is also idle
    Error(String),
}

impl AgentStatus {
    pub fn from_history(history: &History) -> Self {
        history
            .compact
            .as_ref()
            .and_then(|compact| Self::from_entries(&compact.entries))
            .or_else(|| Self::from_entries(history))
            .unwrap_or(Self::Idle)
    }

    fn from_entries(entries: &Entries) -> Option<Self> {
        match entries.last().map(|entry| &entry.message) {
            Some(Message::Assistant(msg)) => Some(msg.finish_reason.clone().into()),
            _ => None,
        }
    }

    pub fn idle(&self) -> bool {
        match self {
            AgentStatus::Compacting => false,
            AgentStatus::InProgress => false,
            AgentStatus::Idle => true,
            AgentStatus::Error(_) => true,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::InProgress | Self::Compacting => "+",
            Self::Idle => " ",
            Self::Error(_) => "!",
        }
    }
}

impl From<AssistantMessageStatus> for AgentStatus {
    fn from(value: AssistantMessageStatus) -> Self {
        match value {
            AssistantMessageStatus::InProgress => Self::InProgress,
            AssistantMessageStatus::Success => Self::Idle,
            AssistantMessageStatus::Error(s) => Self::Error(s),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::llm::provider::assistant::AssistantPool;

    async fn assistant() -> Assistant {
        AssistantPool::from_config(
            &Config::parse(
                r#"
                primary_assistant = ["test"]
                shell_cmd = ["bash", "-c"]

                [sandbox]
                kind = "bwrap"
                bin = "bwrap"
                args = []
                stages = []

                [keymap.cmdline]

                [keymap.normal]

                [keymap.insert]

                [providers.main]
                base_url = "https://api.example.com/v1"

                [assistants.test]
                provider = "main"
                model = "gpt-test"
                "#,
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .assistant("test")
        .unwrap()
    }

    #[tokio::test]
    async fn status_is_not_persisted() {
        let state = AgentState {
            assistant: assistant().await,
            status: AgentStatus::Error("oops".into()),
            topology: Default::default(),
            context: Default::default(),
        };

        let serialized = serde_json::to_value(&state).unwrap();
        assert!(serialized.get("status").is_none());

        crate::llm::provider::assistant::ASSISTANT_POOL
            .get_or_init(|| async {
                AssistantPool::from_config(
                    &Config::parse(
                        r#"
                        primary_assistant = ["test"]
                        shell_cmd = ["bash", "-c"]

                        [sandbox]
                        kind = "bwrap"
                        bin = "bwrap"
                        args = []
                        stages = []

                        [keymap.cmdline]

                        [keymap.normal]

                        [keymap.insert]

                        [providers.main]
                        base_url = "https://api.example.com/v1"

                        [assistants.test]
                        provider = "main"
                        model = "gpt-test"
                        "#,
                    )
                    .unwrap(),
                )
                .await
                .unwrap()
            })
            .await;
        let restored: AgentState = serde_json::from_value(serialized).unwrap();
        assert_eq!(restored.status, AgentStatus::Idle);
    }
}

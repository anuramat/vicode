pub mod compact;
pub mod handle;
pub mod id;
pub mod init;
pub mod router;
pub mod run;
pub mod subagent;
pub mod task;
pub mod tool;
pub mod turn;

use derive_more::Display;
pub use id::*;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;

use crate::agent::handle::AgentEvent;
use crate::agent::handle::ParentEvent;
use crate::agent::handle::TurnResult;
use crate::agent::router::AgentRouterHandle;
use crate::agent::task::manager::AgentTaskManager;
use crate::agent::tool::registry::ToolSchemas;
use crate::forward;
use crate::llm::history::History;
use crate::llm::history::TurnStatus;
use crate::llm::provider::assistant::Assistant;
use crate::project::Project;

#[derive(Debug)]
pub struct Agent {
    pub project: Project,
    pub id: AgentId,
    pub state: AgentState,
    /// router handle for spawning/submitting siblings and children
    pub router: AgentRouterHandle,
    /// pending oneshot for the current turn (set by `SubmitWithCompletion`)
    pub pending_done: Option<oneshot::Sender<TurnResult>>,
    // agent event loop
    pub tx: Sender<AgentEvent>,
    pub rx: Receiver<AgentEvent>,
    /// manages jobs in the agent event loop
    pub tskmgr: AgentTaskManager,
    pub tools: ToolSchemas,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentState {
    /// last emitted status for deduplication of status updates
    #[serde(skip)]
    pub status: AgentStatus,
    pub assistant: Assistant,
    pub visibility: AgentVisibility,
    pub context: AgentContext,
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Default)]
pub enum AgentVisibility {
    Hidden,
    #[default]
    Tab,
}

#[derive(Clone, Debug, PartialEq, Eq, Display)]
pub enum AgentStatus {
    Normal(TurnStatus),
    #[display("compacting: {_0}")]
    Compact(TurnStatus),
}

impl Default for AgentStatus {
    fn default() -> Self {
        Self::Normal(TurnStatus::Idle)
    }
}

impl AgentStatus {
    pub fn turn(&self) -> &TurnStatus {
        match self {
            Self::Normal(t) | Self::Compact(t) => t,
        }
    }

    pub fn idle(&self) -> bool {
        !matches!(self.turn(), TurnStatus::InProgress)
    }

    pub fn label(&self) -> &'static str {
        match self.turn() {
            TurnStatus::InProgress => "+",
            TurnStatus::Idle => " ",
            TurnStatus::Failed(_) => "!",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AgentContext {
    pub commit: String,
    pub history: History,
}

impl Agent {
    forward! {
        history: History = self.state.context.history;
    }

    pub async fn emit(
        &self,
        event: ParentEvent,
    ) -> anyhow::Result<()> {
        self.router
            .app_tx()
            .send(crate::tui::app::AppEvent::ParentEvent(
                self.id.clone(),
                event,
            ))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::config::Config;
    use crate::llm::provider::assistant::AssistantPool;

    async fn assistant() -> Assistant {
        AssistantPool::from_config(
            &Config::parse_with_defaults(
                r#"
                primary_assistant = ["test"]
                shell_cmd = ["bash", "-c"]

                [sandbox]
                kind = "bwrap"
                bin = "bwrap"
                args = []
                stages = []

                [providers.main]
                api = "responses"
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
            status: AgentStatus::Normal(TurnStatus::Failed("oops".into())),
            visibility: AgentVisibility::Tab,
            context: crate::agent::AgentContext {
                commit: "".into(),
                history: History::new("".into()),
            },
        };

        let serialized = serde_json::to_value(&state).unwrap();
        assert!(serialized.get("status").is_none());

        crate::llm::provider::assistant::ASSISTANT_POOL
            .get_or_init(|| async {
                AssistantPool::from_config(
                    &Config::parse_with_defaults(
                        r#"
                        primary_assistant = ["test"]
                        shell_cmd = ["bash", "-c"]

                        [sandbox]
                        kind = "bwrap"
                        bin = "bwrap"
                        args = []
                        stages = []

                        [providers.main]
                        api = "responses"
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
        assert_eq!(restored.status, AgentStatus::default());
    }
}

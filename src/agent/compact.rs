use anyhow::Context;
use anyhow::Result;

use crate::agent::Agent;
use crate::agent::AgentStatus;
use crate::agent::task::sink::TurnType;
use crate::agent::tool::registry::ToolSchemas;
use crate::config::CONFIG;
use crate::llm::history::HistoryUpdate;
use crate::llm::message::Message;
use crate::llm::message::UserMessage;

const COMPACT_PROMPT: &str = "Summarize this conversation for future continuation. Keep concrete user requirements, decisions, constraints, file paths, and unresolved work. Be concise and factual. Output plain text only.";

impl Agent {
    pub async fn dropped(&self) -> usize {
        let window = self.state.assistant.config.window.unwrap_or_default();
        self.state
            .context
            .history
            .compact_dropped(window, CONFIG.compact.target)
    }

    pub async fn init_compact(
        &mut self,
        dropped: usize,
    ) -> Result<()> {
        if dropped == 0 {
            return Ok(());
        }
        let needs_another_turn = self.state.context.history.needs_another_turn();
        let g = self.state.context.history.generation();
        self.handle_history(
            g,
            HistoryUpdate::CompactStart {
                dropped,
                needs_another_turn,
            },
        )
        .await?;
        self.append_compact_prompt()?;
        Ok(())
    }

    pub async fn compact_turn(&mut self) -> Result<()> {
        let messages = self.compact_messages()?;
        self.set_status(AgentStatus::Compacting).await?;
        self.spawn_turn(
            ToolSchemas::empty(),
            self.state.context.history.instructions().to_string(),
            messages,
            TurnType::Compact,
        );
        Ok(())
    }

    // TODO dev message instead?
    fn append_compact_prompt(&mut self) -> Result<()> {
        self.state
            .context
            .history
            .compact
            .as_mut()
            .context("no compact available")?
            .entries
            .push_message(Message::User(UserMessage {
                text: COMPACT_PROMPT.into(),
            }));
        Ok(())
    }

    fn compact_messages(&self) -> Result<Vec<Message>> {
        let compact = self.state.context.history.compact.as_ref();
        let entries = &compact.context("no compact available")?.entries;
        Ok(entries.iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::agent::AgentContext;
    use crate::agent::AgentState;
    use crate::agent::init::channel_parent_sink;
    use crate::agent::task::manager::AgentTaskManager;
    use crate::config::Config;
    use crate::llm::message::Message;
    use crate::llm::message::UserMessage;
    use crate::llm::provider::assistant::Assistant;
    use crate::llm::provider::assistant::AssistantPool;
    use crate::project::Project;
    use crate::project::layout::LayoutTrait;

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

                [keymap.cmdline]

                [keymap.normal]

                [keymap.insert]

                [providers.main]
                base_url = "https://api.example.com/v1"

                [assistants.test]
                provider = "main"
                model = "gpt-test"
                window = 99999
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
    async fn compact_turn_is_noop_when_nothing_is_dropped() {
        let project = Project::new().unwrap();
        let aid = crate::agent::AgentId::from(format!("compact-noop-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(project.agent(&aid))
            .await
            .unwrap();
        let (parent_tx, _parent_rx) = channel(8);
        let (tx, rx) = channel(8);
        let assistant = assistant().await;
        let mut agent = Agent {
            project: project.clone(),
            id: aid.clone(),
            state: AgentState {
                status: Default::default(),
                assistant: assistant.clone(),
                topology: Default::default(),
                context: AgentContext {
                    ..Default::default()
                },
            },
            parent: channel_parent_sink(parent_tx),
            tx,
            rx,
            tskmgr: AgentTaskManager::new(),
            tools: Default::default(),
        };
        agent
            .state
            .context
            .history
            .push_message(Message::User(UserMessage {
                text: "short".into(),
            }));

        agent.init_compact(0).await.unwrap();
        agent.compact_turn().await.err().unwrap();

        assert!(agent.tskmgr.idle());
        assert!(agent.state.context.history.compact.is_none());

        tokio::fs::remove_dir_all(project.agent(&aid))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn append_compact_prompt_adds_prompt_once_per_compact() {
        let project = Project::new().unwrap();
        let aid = crate::agent::AgentId::from(format!("compact-prompt-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(project.agent(&aid))
            .await
            .unwrap();
        let (parent_tx, _parent_rx) = channel(8);
        let (tx, rx) = channel(8);
        let assistant = assistant().await;
        let mut agent = Agent {
            project: project.clone(),
            id: aid.clone(),
            state: AgentState {
                status: Default::default(),
                assistant: assistant.clone(),
                topology: Default::default(),
                context: AgentContext {
                    ..Default::default()
                },
            },
            parent: channel_parent_sink(parent_tx),
            tx,
            rx,
            tskmgr: AgentTaskManager::new(),
            tools: Default::default(),
        };
        agent
            .state
            .context
            .history
            .handle(
                0,
                HistoryUpdate::CompactStart {
                    dropped: 0,
                    needs_another_turn: false,
                },
            )
            .unwrap();

        agent.append_compact_prompt().unwrap();

        let entries = &agent
            .state
            .context
            .history
            .compact
            .as_ref()
            .unwrap()
            .entries;
        assert_eq!(
            entries
                .iter()
                .filter(|entry| {
                    matches!(&entry.message, Message::User(UserMessage { text }) if text == COMPACT_PROMPT)
                })
                .count(),
            1
        );

        tokio::fs::remove_dir_all(project.agent(&aid))
            .await
            .unwrap();
    }
}

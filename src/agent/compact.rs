use anyhow::Result;

use crate::agent::Agent;
use crate::agent::task::sink::TurnType;
use crate::agent::tool::registry::ToolRegistry;
use crate::llm::history::CompactStart;
use crate::llm::history::HistoryUpdate;

impl Agent {
    pub async fn init_compact(
        &mut self,
        n_drop: usize,
    ) -> Result<()> {
        if n_drop == 0 {
            return Ok(());
        }
        let g = self.history().generation();
        self.handle_history(g, HistoryUpdate::CompactStart(CompactStart::new(n_drop)))
            .await?;
        Ok(())
    }

    pub async fn compact_turn(&mut self) -> Result<()> {
        let messages = self.history().compact_turn_input()?;
        self.spawn_turn(
            ToolRegistry::empty(),
            self.history().instructions().to_string(),
            messages,
            TurnType::Compact,
        )
        .await
    }
}

#[cfg(test)]
mod tests {

    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::agent::AgentState;
    use crate::agent::task::manager::AgentTaskManager;
    use crate::config::Config;
    use crate::llm::history::History;
    use crate::llm::history::message::UserMessage;
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

                [providers.main]
                api = "responses"
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
        let project = Project::new_test().unwrap();
        let aid = crate::agent::AgentId::from(format!("compact-noop-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(project.agent(&aid))
            .await
            .unwrap();
        let (tx, rx) = channel(8);
        let assistant = assistant().await;
        let mut agent = Agent {
            project: project.clone(),
            id: aid.clone(),
            state: AgentState {
                status: Default::default(),
                assistant: assistant.clone(),
                max_depth: 1,
                context: crate::agent::AgentContext {
                    commit: "".into(),
                    history: History::new("".into()),
                },
            },
            router: crate::agent::router::AgentRouter::test_handle(),
            pending_done: None,
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
                HistoryUpdate::UserMessage(UserMessage::new("short".into(), 0)),
            )
            .unwrap();

        agent.init_compact(0).await.unwrap();
        agent.compact_turn().await.err().unwrap();

        assert!(agent.tskmgr.idle());
        assert!(!agent.state.context.history.compacting());

        tokio::fs::remove_dir_all(project.agent(&aid))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn append_compact_prompt_adds_prompt_once_per_compact() {
        let project = Project::new_test().unwrap();
        let aid = crate::agent::AgentId::from(format!("compact-prompt-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(project.agent(&aid))
            .await
            .unwrap();
        let (tx, rx) = channel(8);
        let assistant = assistant().await;
        let mut agent = Agent {
            project: project.clone(),
            id: aid.clone(),
            state: AgentState {
                status: Default::default(),
                assistant: assistant.clone(),
                max_depth: 1,
                context: crate::agent::AgentContext {
                    commit: "".into(),
                    history: History::new("".into()),
                },
            },
            router: crate::agent::router::AgentRouter::test_handle(),
            pending_done: None,
            tx,
            rx,
            tskmgr: AgentTaskManager::new(),
            tools: Default::default(),
        };
        agent
            .state
            .context
            .history
            .handle(0, HistoryUpdate::CompactStart(CompactStart::new(0)))
            .unwrap();

        let entries = &agent.history().compact_turn_input().unwrap();

        insta::assert_yaml_snapshot!(entries,
        { "[0].created_at" => "[created_at]" },
        @r#"
        - role: user
          text: "Summarize this conversation for future continuation. Keep concrete user requirements, decisions, constraints, file paths, and unresolved work. Be concise and factual. Output plain text only."
          token_count: 35
          created_at: "[created_at]"
        "#);

        tokio::fs::remove_dir_all(project.agent(&aid))
            .await
            .unwrap();
    }
}

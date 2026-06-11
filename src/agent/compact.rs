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

    use super::*;
    use crate::llm::history::message::UserMessage;

    #[tokio::test]
    async fn compact_turn_is_noop_when_nothing_is_dropped() {
        let (mut agent, _api, _parent_rx) = Agent::fake("compact-noop").await;
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

        assert!(agent.ledger.idle());
        assert!(!agent.state.context.history.compacting());
    }

    #[tokio::test]
    async fn append_compact_prompt_adds_prompt_once_per_compact() {
        let (mut agent, _api, _parent_rx) = Agent::fake("compact-prompt").await;
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
    }
}

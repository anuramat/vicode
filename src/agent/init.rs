use anyhow::Result;
use fs_extra::dir::copy;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;

use crate::agent::handle::ParentEvent;
use crate::agent::handle::ParentMessage;
use crate::agent::task::AgentTaskManager;
use crate::agent::*;
use crate::llm::history::History;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::project::PROJECT;

const CHANNEL_CAPACITY: usize = 100;

impl Agent {
    /// create new agent from scratch
    pub async fn new(
        parent_tx: Sender<ParentMessage>,
        id: AgentId,
        commit: String,
        instructions: String,
    ) -> Result<Self> {
        let state = AgentState::new(id.clone(), commit, instructions).await?;
        Self::from_state(parent_tx, id, state).await
    }

    /// load agent by id from disk
    pub async fn load(
        parent_tx: Sender<ParentMessage>,
        id: AgentId,
    ) -> Result<Self> {
        let state = PROJECT.load_agent_state(&id).await?;
        Self::from_state(parent_tx, id, state).await
    }

    /// shared logic
    async fn from_state(
        parent: Sender<ParentMessage>,
        id: AgentId,
        state: AgentState,
    ) -> Result<Self> {
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        let assistant = ASSISTANT_POOL
            .get()
            .unwrap()
            .assistant(&state.context.assistant_id)?;
        Ok(Self {
            id,
            state,
            parent,
            rx,
            tskmgr: AgentTaskManager::new(),
            tx,
            assistant,
            tools: Default::default(),
        })
    }

    pub async fn save(&self) -> Result<()> {
        self.state.save(&self.id).await
    }

    /// clone agent to given id on manual request from UI
    pub async fn try_duplicate(
        &self,
        aid: AgentId,
    ) -> Result<()> {
        anyhow::ensure!(
            self.tskmgr.pending.is_empty(),
            "cannot duplicate while tasks are running"
        );
        duplicate(&self.id, &aid, &self.state, true).await?;
        self.parent.send((aid, ParentEvent::AttachAgent)).await?;
        Ok(())
    }
}

pub async fn duplicate(
    src_id: &AgentId,
    aid: &AgentId,
    state: &AgentState,
    git: bool,
) -> Result<()> {
    {
        let src = PROJECT.overlay_upper(src_id);
        let dst = PROJECT.overlay_upper(aid);
        let opts = fs_extra::dir::CopyOptions::new().copy_inside(true);
        copy(src, dst, &opts)?;
        tokio::fs::remove_file(PROJECT.overlay_upper(aid).join(".git")).await?;
    }
    PROJECT
        .init_overlay(&state.context.commit, aid, git)
        .await?;
    state.save(aid).await
}

impl AgentState {
    /// init a primary agent from scratch
    async fn new(
        id: AgentId,
        commit: String,
        instructions: String,
    ) -> Result<Self> {
        PROJECT.init_overlay(&commit, &id, true).await?;
        let state = Self {
            topology: AgentTopology {
                children: Vec::new(),
                kind: AgentKind::Primary,
            },
            context: AgentContext {
                commit,
                history: History::new(),
                instructions,
                assistant_id: ASSISTANT_POOL.get().unwrap().next_primary(),
            },
        };
        state.save(&id).await?;
        Ok(state)
    }

    pub async fn save(
        &self,
        id: &AgentId,
    ) -> Result<()> {
        PROJECT.save_agent_state(id, self).await
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::config::Config;
    use crate::llm::provider::assistant::AssistantPool;
    use crate::project::PROJECT;

    #[tokio::test]
    async fn set_assistant_updates_runtime_and_state() {
        let config = Config::parse(
            r#"
            primary_assistant = ["fast"]

            [keymap.cmdline]

            [keymap.normal]

            [keymap.insert]

            [providers.main]
            base_url = "https://api.example.com/v1"
            concurrency = 1
            rpm = 1
            retries = 2
            backoff_ms = 10

            [assistants.fast]
            provider = "main"
            model = "gpt-fast"

            [assistants.deep]
            provider = "main"
            model = "gpt-deep"

            [bash]
            cmd = ["bash", "-c"]

            [bash.bwrap]
            bin = "bwrap"
            args = []
            stages = []
            "#,
        )
        .unwrap();
        let _ = ASSISTANT_POOL.set(AssistantPool::from_config(&config).await.unwrap());

        let aid = AgentId::new().await.unwrap();
        tokio::fs::create_dir_all(PROJECT.agent(&aid))
            .await
            .unwrap();

        let state = AgentState {
            topology: Default::default(),
            context: AgentContext {
                commit: String::new(),
                history: History::new(),
                instructions: String::new(),
                assistant_id: "fast".into(),
            },
        };
        let (parent_tx, _) = channel(1);
        let mut agent = Agent::from_state(parent_tx, aid.clone(), state)
            .await
            .unwrap();

        agent.set_assistant("deep").await.unwrap();

        assert_eq!(agent.state.context.assistant_id, "deep");
        assert_eq!(agent.assistant.config.model.model, "gpt-deep");
        let saved = PROJECT.load_agent_state(&aid).await.unwrap();
        assert_eq!(saved.context.assistant_id, "deep");
        tokio::fs::remove_dir_all(PROJECT.agent(&aid))
            .await
            .unwrap();
    }
}

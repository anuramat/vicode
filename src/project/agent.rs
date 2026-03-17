use anyhow::Result;

use super::Project;
use crate::agent::*;

impl Project {
    pub async fn save_agent_state(
        &self,
        aid: &AgentId,
        data: &AgentState,
    ) -> Result<()> {
        let serialized = serde_json::to_string_pretty(data)?;
        let path = self.agent_state(aid);
        tokio::fs::write(path, serialized).await?;
        Ok(())
    }

    pub async fn load_agent_state(
        &self,
        aid: &AgentId,
    ) -> Result<AgentState> {
        let path = self.agent_state(aid);
        let serialized = tokio::fs::read_to_string(path).await?;
        Ok(serde_json::from_str(&serialized)?)
    }

    pub async fn delete_agent(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        self.unmount(aid).await?;
        Ok(tokio::fs::remove_dir_all(self.agent(aid)).await?)
    }

    pub async fn agent_id_exists(
        &self,
        aid: &AgentId,
    ) -> Result<bool> {
        Ok(tokio::fs::try_exists(self.agent(aid)).await?)
    }
}

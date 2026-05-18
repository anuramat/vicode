use std::path::PathBuf;

use anyhow::Result;

use crate::agent::id::AgentId;
use crate::agent::router::AgentRouterHandle;
use crate::config::Config;
use crate::project::Project;
use crate::project::layout::LayoutTrait;
use crate::sandbox::SandboxRunner;

#[derive(Clone, Debug)]
pub struct ToolRuntimeContext {
    pub agent_id: AgentId,
    pub project: Project,
    pub router: AgentRouterHandle,
}

impl ToolRuntimeContext {
    pub fn new(
        agent_id: AgentId,
        project: Project,
        router: AgentRouterHandle,
    ) -> Self {
        Self {
            agent_id,
            project,
            router,
        }
    }

    pub fn workdir(&self) -> PathBuf {
        self.project.agent_workdir(&self.agent_id)
    }

    pub fn changes_dir(&self) -> PathBuf {
        self.project.agent_changes_dir(&self.agent_id)
    }

    pub fn sandbox_runner(&self) -> Result<SandboxRunner> {
        Ok(self
            .project
            .sandbox_runner(self.workdir(), self.project.gitdir()?))
    }

    pub fn config(&self) -> &Config {
        self.project.config()
    }
}

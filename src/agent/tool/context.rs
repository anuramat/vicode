use std::path::PathBuf;

use anyhow::Result;

use crate::agent::id::AgentId;
use crate::agent::router::Router;
use crate::agent::task::sink::TaskSink;
use crate::config::Config;
use crate::llm::history::History;
use crate::project::Project;
use crate::sandbox::SandboxRunner;

#[derive(Clone, Debug)]
pub struct ToolRuntimeContext {
    pub agent_id: AgentId,
    pub project: Project,
    pub router: Router,
    /// streams output chunks back to the agent
    pub sink: TaskSink,
    /// parent history for `spawn` calls with inherit=true
    pub inherited_history: Option<History>,
}

impl ToolRuntimeContext {
    pub fn new(
        agent_id: AgentId,
        project: Project,
        router: Router,
        sink: TaskSink,
        inherited_history: Option<History>,
    ) -> Self {
        Self {
            agent_id,
            project,
            router,
            sink,
            inherited_history,
        }
    }

    pub fn workdir(&self) -> PathBuf {
        self.project.agent_workdir(&self.agent_id)
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

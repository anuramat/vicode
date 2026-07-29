use std::path::PathBuf;

use anyhow::Result;

use crate::agent::id::AgentId;
use crate::agent::router::AgentRouterHandle;
use crate::agent::task::sink::OutputSink;
use crate::config::Config;
use crate::llm::history::History;
use crate::project::Project;
use crate::sandbox::SandboxRunner;

#[derive(Clone, Debug)]
pub struct ToolRuntimeContext {
    pub agent_id: AgentId,
    pub project: Project,
    pub router: AgentRouterHandle,
    /// per-call incremental output stream (§2.4a)
    pub output: OutputSink,
    /// `spawn` only: the core-captured history snapshot (§2.2/§2.3)
    pub capture: Option<History>,
}

impl ToolRuntimeContext {
    pub fn new(
        agent_id: AgentId,
        project: Project,
        router: AgentRouterHandle,
        output: OutputSink,
        capture: Option<History>,
    ) -> Self {
        Self {
            agent_id,
            project,
            router,
            output,
            capture,
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

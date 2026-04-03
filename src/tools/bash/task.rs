use std::os::unix::process::ExitStatusExt;

use anyhow::Result;

use crate::agent::tool::traits::*;
use crate::config::CONFIG;
use crate::project::layout::LayoutTrait;
use crate::sandbox::Sandbox;
use crate::tools::bash::BashArguments;
use crate::tools::bash::BashContext;
use crate::tools::bash::BashResult;

impl ToolContext<BashArguments> for BashContext {
    fn prepare(
        _args: &BashArguments,
        agent: &crate::agent::Agent,
    ) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(Self {
            runner: CONFIG.sandbox.runner(
                agent.project.agent_workdir(&agent.id),
                agent.project.gitdir()?,
            ),
        })
    }
}

#[async_trait::async_trait]
impl Function<BashContext, (), BashResult> for BashArguments {
    async fn call(
        &self,
        ctx: BashContext,
    ) -> Result<(BashResult, ())> {
        let runner = &ctx.runner;

        let std::process::Output {
            stdout,
            stderr,
            status,
        } = runner.exec(self.command.clone()).await?;

        let result = BashResult {
            stdout: String::from_utf8_lossy(&stdout).into(),
            stderr: String::from_utf8_lossy(&stderr).into(),
            exit_status: status.code(),
            signal: status.signal(),
        };

        Ok((result, ()))
    }
}

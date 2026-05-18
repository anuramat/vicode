use std::os::unix::process::ExitStatusExt;

use anyhow::Result;

use crate::agent::tool::context::ToolRuntimeContext;
use crate::agent::tool::traits::Function;
use crate::tools::bash::BashArguments;
use crate::tools::bash::BashResult;

#[async_trait::async_trait]
impl Function<(), BashResult> for BashArguments {
    async fn call(
        &self,
        ctx: ToolRuntimeContext,
    ) -> Result<(BashResult, ())> {
        let runner = ctx.sandbox_runner()?;
        let shell_cmd = ctx.config().shell_cmd.clone();

        let std::process::Output {
            stdout,
            stderr,
            status,
        } = runner.exec(shell_cmd, self.command.clone()).await?;

        let result = BashResult {
            stdout: String::from_utf8_lossy(&stdout).into(),
            stderr: String::from_utf8_lossy(&stderr).into(),
            exit_status: status.code(),
            signal: status.signal(),
        };

        Ok((result, ()))
    }
}

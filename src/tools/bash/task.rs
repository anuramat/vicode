use std::os::unix::process::ExitStatusExt;

use anyhow::Context;
use anyhow::Result;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;

use crate::agent::task::sink::TaskSink;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::agent::tool::traits::Function;
use crate::sandbox::SandboxRunner;
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
        let result = exec_streaming(&runner, shell_cmd, self.command.clone(), &ctx.sink).await?;
        Ok((result, ()))
    }

    fn compose(
        result: Option<BashResult>,
        output: String,
    ) -> Option<BashResult> {
        Some(BashResult {
            output,
            ..result.unwrap_or_default()
        })
    }
}

/// incremental exec: chunks stream through the sink as they arrive (stdout
/// and stderr interleaved); the output the call streams into history is the
/// authoritative text, the return carries only the exit status
pub async fn exec_streaming(
    runner: &SandboxRunner,
    shell_cmd: Vec<String>,
    script: String,
    sink: &TaskSink,
) -> Result<BashResult> {
    let mut child = runner.spawn(shell_cmd, script)?;
    let out = child.stdout.take().context("no stdout pipe")?;
    let err = child.stderr.take().context("no stderr pipe")?;
    // a failed read fails the call: the child is killed on drop instead of
    // blocking on a pipe nobody reads
    let ((), (), status) = tokio::try_join!(drain(out, sink), drain(err, sink), child.wait())?;
    Ok(BashResult {
        output: String::new(),
        exit_status: status.code(),
        signal: status.signal(),
    })
}

/// read a pipe to EOF, streaming each chunk to the sink (lossy); a character
/// cut off by a chunk boundary is held back until the next chunk completes it
async fn drain(
    mut reader: impl AsyncRead + Unpin,
    sink: &TaskSink,
) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    let mut pending = Vec::new();
    loop {
        let n = reader.read(&mut buf).await?;
        pending.extend_from_slice(&buf[..n]);
        // at EOF, a cut-off character stays cut off
        let held = if n == 0 { 0 } else { cut_off(&pending) };
        let rest = pending.split_off(pending.len() - held);
        let chunk = std::mem::replace(&mut pending, rest);
        if !chunk.is_empty() {
            sink.output(String::from_utf8_lossy(&chunk).into());
        }
        if n == 0 {
            return Ok(());
        }
    }
}

/// length of the multi-byte character cut off at the end of `bytes`, if any:
/// the last non-continuation byte starts the last character, and its leading
/// ones give the character's length
fn cut_off(bytes: &[u8]) -> usize {
    let tail = &bytes[bytes.len().saturating_sub(3)..];
    match tail.iter().rposition(|b| b & 0xC0 != 0x80) {
        Some(i) if tail[i].leading_ones() as usize > tail.len() - i => tail.len() - i,
        _ => 0,
    }
}

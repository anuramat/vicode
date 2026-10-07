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
        result: &mut BashResult,
        streamed: String,
    ) {
        result.output = streamed;
    }
}

/// incremental exec: chunks stream through the sink as they arrive (stdout
/// and stderr interleaved); the partial the agent accumulates is the
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
    let ((), (), status) = tokio::join!(drain(out, sink), drain(err, sink), child.wait());
    let status = status?;
    Ok(BashResult {
        output: String::new(),
        exit_status: status.code(),
        signal: status.signal(),
    })
}

/// read a pipe to EOF, streaming each chunk to the sink (lossy per chunk)
async fn drain(
    mut reader: impl AsyncRead + Unpin,
    sink: &TaskSink,
) {
    let mut buf = [0u8; 8192];
    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            break;
        }
        sink.output(String::from_utf8_lossy(&buf[..n]).into());
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::agent::event::AgentEvent;
    use crate::agent::task::Task;
    use crate::agent::task::Tasks;

    #[tokio::test]
    async fn exec_streams_chunks_and_returns_status_only() {
        let runner = SandboxRunner {
            bin: "bash".into(),
            args: vec![],
            cwd: std::env::temp_dir(),
        };
        let (tx, mut rx) = unbounded_channel();
        let tid = Tasks::default().register(Task::turn());
        let sink = TaskSink::new(tid, tx);

        let mut result = exec_streaming(
            &runner,
            vec!["-c".into()],
            "echo one; echo two >&2; exit 3".into(),
            &sink,
        )
        .await
        .unwrap();

        // the text lives in the stream, not the return
        similar_asserts::assert_eq!(
            result,
            BashResult {
                output: String::new(),
                exit_status: Some(3),
                signal: None,
            }
        );
        // every chunk reached the sink, tagged with the task id; composing
        // the accumulated stream fills the output text
        let mut streamed = String::new();
        while let Ok(event) = rx.try_recv() {
            let AgentEvent::Output(id, chunk) = event else {
                panic!("expected output, got {event:?}");
            };
            assert_eq!(id, tid);
            streamed.push_str(&chunk);
        }
        assert_eq!(streamed.len(), "one\ntwo\n".len());
        BashArguments::compose(&mut result, streamed.clone());
        assert_eq!(result.output, streamed);
    }
}

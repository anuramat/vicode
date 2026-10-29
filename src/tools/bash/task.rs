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

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;
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
        let tid = Tasks::held().register(Task::Turn);
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

    /// a reader that fails on every read
    struct Broken;

    impl AsyncRead for Broken {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other("broken pipe")))
        }
    }

    /// what `reader` streams through `drain`, and how the drain ended
    async fn drained(reader: impl AsyncRead + Unpin) -> (String, Result<(), String>) {
        let (tx, mut rx) = unbounded_channel();
        let sink = TaskSink::new(Tasks::held().register(Task::Turn), tx);
        let result = drain(reader, &sink).await.map_err(|e| e.to_string());
        let mut streamed = String::new();
        while let Ok(AgentEvent::Output(_, chunk)) = rx.try_recv() {
            streamed.push_str(&chunk);
        }
        (streamed, result)
    }

    /// a read error ends the drain with that error, after streaming what
    /// came before it
    #[tokio::test]
    async fn drain_surfaces_read_errors() {
        assert_eq!(
            drained(b"partial".chain(Broken)).await,
            ("partial".into(), Err("broken pipe".into()))
        );
    }

    /// characters split across reads come out whole; invalid bytes and a
    /// character cut off by EOF stay lossy
    #[tokio::test]
    async fn drain_joins_characters_split_across_reads() {
        assert_eq!(
            [
                drained(b"\xC3".chain(&b"\xA9!"[..])).await,
                drained(b"a\xF0".chain(&b"\x9F\x98"[..]).chain(&b"\x80"[..])).await,
                drained(b"\xFF".chain(&b"\xC3"[..])).await,
            ],
            [
                ("\u{e9}!".into(), Ok(())),
                ("a\u{1F600}".into(), Ok(())),
                ("\u{FFFD}\u{FFFD}".into(), Ok(())),
            ]
        );
    }
}

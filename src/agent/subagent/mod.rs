pub mod replica;
/// SLOP `result` module is vibecoded
#[allow(deprecated, clippy::pedantic, clippy::nursery, clippy::style)]
pub mod result;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio::sync::oneshot;

use super::handle::ExternalEvent;
use crate::agent::Agent;
use crate::agent::AgentEvent;
use crate::agent::AgentStatus;
use crate::agent::handle::ParentEvent;
use crate::agent::handle::UserPrompt;
use crate::agent::id::AgentId;
use crate::agent::init::channel_parent_sink;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentResult {
    pub output: String,
    pub diff: String,
}

#[derive(Debug)]
pub struct SubagentHandle {
    pub id: AgentId,
    output: oneshot::Receiver<Result<SubagentResult>>,
}

impl SubagentHandle {
    pub async fn wait(self) -> Result<SubagentResult> {
        self.output
            .await
            .context("failed to receive subagent output")?
    }
}

pub async fn spawn(
    parent: &Agent,
    prompt: String,
    inherit_context: bool,
) -> Result<SubagentHandle> {
    let (parent_tx, parent_rx) = channel(100);
    let sink = channel_parent_sink(parent_tx);
    let agent = parent.subagent(sink, inherit_context).await?;

    let child_tx = agent.tx.clone();
    let aid = agent.id.clone();
    let generation = agent.state.context.history.generation();
    agent.spawn();

    let (output_tx, output) = oneshot::channel();
    tokio::spawn(async move {
        let result = run_child(generation, prompt, child_tx, parent_rx).await;
        _ = output_tx.send(result);
    });
    Ok(SubagentHandle { id: aid, output })
}

async fn run_child(
    generation: u64,
    prompt: String,
    child_tx: Sender<AgentEvent>,
    mut parent_rx: Receiver<ParentEvent>,
) -> Result<SubagentResult> {
    child_tx
        .send(AgentEvent::External(ExternalEvent::Submit(UserPrompt {
            text: prompt,
            multiplier: 1,
            generation,
        })))
        .await?;
    loop {
        match parent_rx.recv().await {
            Some(event) => match event {
                ParentEvent::SubagentDone(s) => return Ok(s),
                ParentEvent::StatusUpdate(AgentStatus::Error(err)) => {
                    anyhow::bail!("subagent error: {err}");
                }
                _ => {}
            },
            None => anyhow::bail!("subagent channel closed before turn completion"),
        }
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;
    use tokio::sync::mpsc::channel;

    use super::*;

    #[tokio::test]
    async fn run_child_submits_prompt_with_generation() {
        let (child_tx, mut child_rx) = channel(1);
        let (parent_tx, parent_rx) = channel(1);
        let expected = SubagentResult {
            output: "done".into(),
            diff: "diff --git".into(),
        };
        let task = tokio::spawn(run_child(0, "hello".into(), child_tx, parent_rx));

        let event = child_rx.recv().await.unwrap();
        insta::assert_debug_snapshot!(event, @r#"
        External(
            Submit(
                UserPrompt {
                    text: "hello",
                    multiplier: 1,
                    generation: 0,
                },
            ),
        )
        "#);

        parent_tx
            .send(ParentEvent::SubagentDone(expected.clone()))
            .await
            .unwrap();
        assert_eq!(task.await.unwrap().unwrap(), expected);
    }

    #[tokio::test]
    async fn run_child_returns_terminal_error_status() {
        let (child_tx, mut child_rx) = channel(1);
        let (parent_tx, parent_rx) = channel(2);
        let task = tokio::spawn(run_child(0, String::new(), child_tx, parent_rx));

        child_rx.recv().await.unwrap();
        parent_tx.send(ParentEvent::Error("oops".into())).await.unwrap();
        tokio::task::yield_now().await;
        assert!(!task.is_finished());

        parent_tx
            .send(ParentEvent::StatusUpdate(AgentStatus::Error("oops".into())))
            .await
            .unwrap();
        insta::assert_snapshot!(task.await.unwrap().unwrap_err().to_string(), @"subagent error: oops");
    }
}

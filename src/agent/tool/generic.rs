use std::fmt::Debug;

use anyhow::Result;
use serde::Serialize;

use super::traits::*;
use crate::agent::Agent;

// TODO can we drop Clone for this and tctx?
// NOTE we have to write explicit bounds because serde heuristics break on Option<T> -- they incorrectly require T to implement Default
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(bound(
    serialize = "TArgs: serde::Serialize, TResult: serde::Serialize, TMeta: serde::Serialize",
    deserialize = "TArgs: serde::de::DeserializeOwned, TResult: serde::de::DeserializeOwned, TMeta: serde::de::DeserializeOwned",
))]
pub struct GenericTask<TArgs, TCtx, TMeta, TResult> {
    pub arguments: Option<TArgs>,
    #[serde(skip)]
    pub context: Option<TCtx>,
    pub meta: Option<TMeta>,
    /// None if the task has not been run yet; Some(Err) if there was a runtime error (we still send the message to the LLM)
    pub output: Option<Result<TResult, String>>,
}

#[async_trait::async_trait]
impl<TArgs, TCtx, TMeta, TResult> ToolCall for GenericTask<TArgs, TCtx, TMeta, TResult>
where
    TArgs: Serialize + Function<TCtx, TMeta, TResult> + Send + Sync,
    TResult: Serialize + Send + Sync,
    TMeta: Serialize + Send + Sync,
    TCtx: Send + Sync + ToolContext<TArgs>,
{
    fn arguments(&self) -> String {
        serde_json::to_string(&self.arguments).expect("could not serialize arguments")
    }

    fn output(&self) -> Option<String> {
        self.output
            .as_ref()
            .map(|x| match x {
                Ok(res) => serde_json::to_string(&res),
                Err(err) => {
                    let value = serde_json::json!({
                        "error": err,
                    });
                    serde_json::to_string(&value)
                }
            })
            .transpose()
            .expect("could not serialize output")
    }

    fn prepare(
        &mut self,
        agent: &Agent,
    ) -> Result<()> {
        self.context = TCtx::prepare(self.arguments.as_ref().unwrap(), agent)
            .unwrap()
            .into();
        Ok(())
    }

    async fn run(&mut self) {
        let args = self.arguments.as_ref().unwrap();
        let ctx = self.context.take().unwrap();
        match args.call(ctx).await {
            Ok((result, meta)) => {
                self.meta = Some(meta);
                self.output = Some(Ok(result));
            }
            Err(e) => {
                self.output = Some(Err(e.to_string()));
            }
        }
    }
}

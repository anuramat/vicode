use std::fmt::Debug;

use serde::Serialize;

use super::traits::Function;
use super::traits::ToolCall;
use crate::agent::tool::context::ToolRuntimeContext;

// NOTE we have to write explicit bounds because serde heuristics break on Option<T> -- they incorrectly require T to implement Default
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(bound(
    serialize = "TArgs: serde::Serialize, TResult: serde::Serialize, TMeta: serde::Serialize",
    deserialize = "TArgs: serde::de::DeserializeOwned, TResult: serde::de::DeserializeOwned, TMeta: serde::de::DeserializeOwned",
))]
pub struct GenericTask<TArgs, TMeta, TResult> {
    pub arguments: Option<TArgs>,
    pub meta: Option<TMeta>,
    /// None if the task has not been run yet; Some(Err) if there was a runtime error (we still send the message to the LLM)
    pub output: Option<Result<TResult, String>>,
}

#[async_trait::async_trait]
impl<TArgs, TMeta, TResult> ToolCall for GenericTask<TArgs, TMeta, TResult>
where
    TArgs: Serialize + Function<TMeta, TResult> + Send + Sync,
    TResult: Serialize + Send + Sync,
    TMeta: Serialize + Send + Sync,
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

    async fn run(
        &mut self,
        ctx: ToolRuntimeContext,
    ) {
        let args = self.arguments.as_ref().unwrap();
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

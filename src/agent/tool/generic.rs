use std::fmt::Debug;

use serde::Serialize;

use super::traits::Function;
use super::traits::ToolCall;
use crate::agent::tool::context::ToolRuntimeContext;

/// how a call resolved; a failed call keeps any partial result as a `T`, so it
/// reports the fields of a successful call, at the same paths, plus `error`
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolOutcome<T> {
    pub result: Option<T>,
    pub error: Option<String>,
}

impl<T> ToolOutcome<T> {
    pub const fn ok(result: T) -> Self {
        Self {
            result: Some(result),
            error: None,
        }
    }

    pub const fn failed(error: String) -> Self {
        Self {
            result: None,
            error: Some(error),
        }
    }
}

// NOTE we have to write explicit bounds because serde heuristics break on Option<T> -- they incorrectly require T to implement Default
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(bound(
    serialize = "TArgs: serde::Serialize, TResult: serde::Serialize, TMeta: serde::Serialize",
    deserialize = "TArgs: serde::de::DeserializeOwned, TResult: serde::de::DeserializeOwned, TMeta: serde::de::DeserializeOwned",
))]
pub struct GenericTask<TArgs, TMeta, TResult> {
    pub arguments: Option<TArgs>,
    pub meta: Option<TMeta>,
    /// None if the task has not been run yet; a runtime error is an outcome with `error` set (we still send the message to the LLM)
    pub output: Option<ToolOutcome<TResult>>,
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
            .map(|outcome| {
                let Some(error) = &outcome.error else {
                    return serde_json::to_string(&outcome.result);
                };
                let mut value = serde_json::to_value(&outcome.result)?;
                value["error"] = error.as_str().into();
                serde_json::to_string(&value)
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
                self.output = Some(ToolOutcome::ok(result));
            }
            Err(e) => {
                self.output = Some(ToolOutcome::failed(e.to_string()));
            }
        }
    }

    fn fail_unresolved(
        &mut self,
        msg: &str,
    ) -> bool {
        let unresolved = self.output.is_none();
        if unresolved {
            self.output = Some(ToolOutcome::failed(msg.to_string()));
        }
        unresolved
    }

    fn inherit_history(&self) -> bool {
        self.arguments
            .as_ref()
            .is_some_and(Function::inherit_history)
    }

    fn compose(
        &mut self,
        streamed: String,
    ) {
        if let Some(outcome) = &mut self.output
            && !streamed.is_empty()
        {
            outcome.result = TArgs::compose(outcome.result.take(), streamed);
        }
    }
}

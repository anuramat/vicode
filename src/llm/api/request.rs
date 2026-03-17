use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use backon::ExponentialBuilder;
use backon::Retryable;
use backon::TokioSleeper;
use tokio::sync::AcquireError;
use tracing::instrument;

use crate::agent::tool::registry::ToolSchemas;
use crate::llm::api::assistant::Assistant;
use crate::llm::api::backend::AssistantStream;
use crate::llm::history::History;

impl Assistant {
    #[instrument(skip(self, history, tools, instructions))]
    pub async fn stream_turn(
        &self,
        instructions: String,
        history: History,
        tools: ToolSchemas,
    ) -> Result<AssistantStream> {
        retry(
            || async {
                self.api.ratelimiter.until_ready().await;
                let permit = self.api.semaphore.clone().acquire_owned().await?;
                self.api
                    .backend
                    .stream(
                        permit,
                        self.config.model.clone(),
                        instructions.clone(),
                        history.clone(),
                        tools.clone(),
                    )
                    .await
            },
            self.api.config.backoff_ms,
            self.api.config.retries,
        )
        .await
    }
}

async fn retry<F, Fut, T>(
    op: F,
    backoff_ms: u64,
    retries: usize,
) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let builder = ExponentialBuilder::default()
        .with_min_delay(Duration::from_millis(backoff_ms))
        .with_jitter()
        .with_max_times(retries);
    op.retry(builder)
        .sleep(TokioSleeper)
        .when(|e| !e.is::<AcquireError>())
        .await
}

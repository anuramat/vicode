use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream;
use governor::Quota;
use governor::RateLimiter;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::Semaphore;

use super::Api;
use super::AssistantStream;
use super::StartedAssistantStream;
use crate::agent::tool::registry::ToolRegistry;
use crate::config::ModelConfig;
use crate::llm::history::AssistantEvent;
use crate::llm::history::message::Message;
use crate::llm::provider::ApiKeyProvider;
use crate::llm::provider::Provider;
use crate::llm::provider::ProviderConfig;
use crate::llm::provider::RateLimits;
use crate::llm::provider::assistant::Assistant;
use crate::utils::now;

/// scripted in-memory `Api`: every `stream` call replays the next scripted turn
#[derive(Debug, Default)]
pub struct FakeApi {
    turns: Mutex<VecDeque<FakeTurn>>,
    /// messages passed to each `stream` call, in order
    requests: Mutex<Vec<Vec<Message>>>,
}

#[derive(Debug)]
struct FakeTurn {
    events: Vec<AssistantEvent>,
    /// keep the stream open (pending forever) after replaying all events
    hang: bool,
}

impl FakeApi {
    pub fn script_turn(
        &self,
        events: Vec<AssistantEvent>,
    ) {
        self.turns.lock().unwrap().push_back(FakeTurn {
            events,
            hang: false,
        });
    }

    pub fn script_hanging_turn(
        &self,
        events: Vec<AssistantEvent>,
    ) {
        self.turns
            .lock()
            .unwrap()
            .push_back(FakeTurn { events, hang: true });
    }

    pub fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl Api for FakeApi {
    async fn stream(
        &self,
        permit: OwnedSemaphorePermit,
        _model: ModelConfig,
        _instructions: String,
        messages: Vec<Message>,
        _tools: ToolRegistry,
    ) -> Result<StartedAssistantStream> {
        self.requests.lock().unwrap().push(messages);
        let turn = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .context("no scripted turn")?;
        let events = stream::iter(turn.events.into_iter().map(Ok));
        let stream: AssistantStream = if turn.hang {
            Box::pin(events.chain(stream::pending()))
        } else {
            Box::pin(events)
        };
        Ok(StartedAssistantStream {
            started_at: now(),
            stream: Box::pin(stream.map(move |event| {
                // reference the permit so it's captured and held for the stream's lifetime
                let _permit = &permit;
                event
            })),
        })
    }
}

impl Assistant {
    /// assistant backed by a scripted `FakeApi`, no retries; the test keeps the
    /// returned handle to script turns and inspect requests
    pub fn fake() -> (Self, Arc<FakeApi>) {
        let api = Arc::new(FakeApi::default());
        let limits = RateLimits {
            retries: 0,
            backoff_ms: 1,
            ..Default::default()
        };
        let provider = Provider {
            ratelimiter: RateLimiter::direct(Quota::per_minute(limits.rpm.try_into().unwrap())),
            semaphore: Arc::new(Semaphore::new(limits.concurrency)),
            config: ProviderConfig::ChatCompletions(ApiKeyProvider {
                limits,
                ..Default::default()
            }),
            api: api.clone(),
        };
        let assistant = Self {
            id: "test".into(),
            provider: Arc::new(provider),
            config: ModelConfig {
                model: "fake-model".into(),
                effort: None,
                window: None,
            },
        };
        (assistant, api)
    }
}

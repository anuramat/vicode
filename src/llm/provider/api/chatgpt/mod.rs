pub mod auth;
pub mod cli;
pub mod error;
pub mod stream;

#[cfg(test)]
pub mod test_support;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
pub use auth::ChatgptAuthManager;
pub use auth::provider_auth;
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::tool::registry::ToolSchemas;
use crate::config::ApiCompatConfig;
use crate::config::ModelConfig;
use crate::llm::history::message::Message;
use crate::llm::provider::api::Api;
use crate::llm::provider::api::StartedAssistantStream;
use crate::llm::provider::api::responses;

pub const CHATGPT_AUTH_TYPE: &str = "chatgpt_oauth";
pub const CHATGPT_AUTH_VERSION: usize = 1;
pub const CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OAUTH_ISSUER: &str = "https://auth.openai.com";
pub const LOGIN_REFRESH_WINDOW_MS: u64 = 5 * 60 * 1000;
pub const POLL_SAFETY_MARGIN_SECS: u64 = 1;
pub const ORIGINATOR: &str = "vicode";
pub const USER_AGENT_VALUE: &str = concat!("vicode/", env!("CARGO_PKG_VERSION"));

#[derive(Debug)]
pub struct ChatgptApi {
    auth: ChatgptAuthManager,
}

impl ChatgptApi {
    pub const fn new(auth: ChatgptAuthManager) -> Self {
        Self { auth }
    }
}

#[async_trait]
impl Api for ChatgptApi {
    async fn stream(
        &self,
        permit: OwnedSemaphorePermit,
        model: ModelConfig,
        instructions: String,
        messages: Vec<Message>,
        tools: ToolSchemas,
    ) -> Result<StartedAssistantStream> {
        let compat = ApiCompatConfig::default();
        let inner = stream::run(&self.auth, CHATGPT_BASE_URL, || {
            responses::request(
                model.clone(),
                instructions.clone(),
                messages.clone(),
                tools.clone(),
                &compat,
            )
        })
        .await
        .context("ChatGPT streaming request failed")?;
        Ok(responses::started_stream(permit, inner))
    }
}

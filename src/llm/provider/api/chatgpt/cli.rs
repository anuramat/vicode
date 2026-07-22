use anyhow::Context;
use anyhow::Result;

use super::ChatgptAuthManager;
use crate::config::Config;

#[derive(clap::Subcommand)]
pub enum ChatgptCommand {
    /// Login to a ChatGPT-authenticated provider
    Login(ProviderCommand),
    /// Logout from a ChatGPT-authenticated provider
    Logout(ProviderCommand),
    /// Show auth status for a ChatGPT-authenticated provider
    Status(ProviderCommand),
}

#[derive(clap::Args)]
pub struct ProviderCommand {
    pub provider_id: String,
}

impl ChatgptCommand {
    pub async fn run(&self) -> Result<()> {
        match self {
            Self::Login(args) => auth_manager(&args.provider_id)?.login_headless().await?,
            Self::Logout(args) => auth_manager(&args.provider_id)?.logout()?,
            Self::Status(args) => {
                let status = auth_manager(&args.provider_id)?.status()?;
                println!("provider_id: {}", status.provider_id);
                println!("auth_mode: chatgpt");
                println!("logged_in: {}", if status.logged_in { "yes" } else { "no" });
                if let Some(expired) = status.expired {
                    println!("expired: {}", if expired { "yes" } else { "no" });
                }
                if let Some(expires_at) = status.expires_at_unix_ms {
                    println!("expires_at_unix_ms: {expires_at}");
                }
                if let Some(account_id) = status.account_id {
                    println!("account_id: {account_id}");
                }
            }
        }
        Ok(())
    }
}

fn auth_manager(provider_id: &str) -> Result<ChatgptAuthManager> {
    let config = Config::load()?;
    let provider = config
        .providers
        .get(provider_id)
        .with_context(|| format!("unknown provider '{provider_id}'"))?;
    anyhow::ensure!(
        provider.is_chatgpt(),
        "provider '{provider_id}' is not configured with auth = \"chatgpt\""
    );
    ChatgptAuthManager::new(provider_id)
}

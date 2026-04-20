use anyhow::Result;
use clap::Parser;

use crate::config::Config;
use crate::llm::provider::api::chatgpt::cli::ChatgptCommand;

#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Manage config
    #[command(subcommand)]
    Config(ConfigCommand),
    #[allow(clippy::doc_markdown)]
    /// Manage ChatGPT authentication
    #[command(subcommand)]
    Chatgpt(ChatgptCommand),
}

#[derive(clap::Subcommand)]
pub enum ConfigCommand {
    /// Show effective config
    Show,
}

impl Command {
    pub async fn run(&self) -> Result<()> {
        match self {
            Self::Config(ConfigCommand::Show) => println!("{}", Config::load()?),
            Self::Chatgpt(cmd) => cmd.run().await?,
        }
        Ok(())
    }
}

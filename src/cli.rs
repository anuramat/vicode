use clap::Parser;

use crate::config::Config;

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
}

#[derive(clap::Subcommand)]
pub enum ConfigCommand {
    /// Show effective config
    Show,
}

impl Command {
    pub fn run(&self) {
        match self {
            Command::Config(ConfigCommand::Show) => println!("{}", Config::load().unwrap()),
        }
    }
}

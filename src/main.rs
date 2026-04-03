#![feature(min_specialization)]
#![feature(type_alias_impl_trait)]
#![feature(exit_status_error)]
#![feature(iterator_try_collect)]

mod agent;
mod cli;
mod config;
mod git;
mod id;
mod llm;
mod project;
mod sandbox;
mod tools;
mod tui;

use anyhow::Result;
use clap::Parser;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tui::app::App;

use crate::cli::Cli;
use crate::project::PROJECT;
use crate::project::layout::LayoutTrait;

fn init_tracing() -> Result<WorkerGuard> {
    let dir = config::DIRS.create_state_directory("")?;
    let appender = tracing_appender::rolling::never(&dir, format!("{}.log", PROJECT.id()));
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::from_default_env();
    fmt().with_env_filter(filter).with_writer(writer).init();
    Ok(guard)
}

#[tokio::main]
async fn main() {
    tracing::debug!("starting main");
    let _guard = init_tracing().unwrap();
    match Cli::parse().command {
        Some(command) => command.run(),
        None => {
            let result = App::launch().await;
            App::reset_terminal();
            result.unwrap();
        }
    }
}

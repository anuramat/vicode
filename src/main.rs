#![feature(min_specialization)]
#![feature(type_alias_impl_trait)]
#![feature(exit_status_error)]
#![feature(iterator_try_collect)]

mod agent;
mod bwrap;
mod config;
mod git;
mod id;
mod llm;
mod project;
mod tools;
mod tui;

use anyhow::Result;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tui::app::App;

use crate::project::PROJECT;

fn init_tracing() -> Result<WorkerGuard> {
    let dir = config::DIRS.create_state_directory("")?;
    let appender = tracing_appender::rolling::never(&dir, format!("{}.log", PROJECT.id));
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::from_default_env();
    fmt().with_env_filter(filter).with_writer(writer).init();
    Ok(guard)
}

#[tokio::main]
async fn main() {
    let _guard = init_tracing().unwrap();
    let result = App::launch().await;
    ratatui::restore();
    result.unwrap();
}

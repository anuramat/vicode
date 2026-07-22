#![cfg_attr(test, allow(clippy::pedantic, clippy::nursery, clippy::style))]

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("unsupported platform");

mod agent;
mod cli;
mod config;
mod deps;
mod git;
mod id;
mod llm;
mod macros;
mod project;
mod sandbox;
mod tools;
mod tui;
mod utils;

use std::panic::AssertUnwindSafe;

use anyhow::anyhow;
use clap::Parser;
use futures::FutureExt;
use tui::app::App;

use crate::cli::Cli;
use crate::config::Config;
use crate::utils::panic_message;

#[tokio::main]
async fn main() {
    if let Some(command) = Cli::parse().command {
        if let Err(err) = command.run().await {
            fatal(&err);
        }
        return;
    }

    let config = Config::load().unwrap_or_else(|err| fatal(&err));
    let result = AssertUnwindSafe(App::launch(config))
        .catch_unwind()
        .await
        .unwrap_or_else(|panic| Err(anyhow!("app panicked: {}", panic_message(&*panic))));
    App::reset_terminal();
    if let Err(err) = result {
        fatal(&err);
    }
}

fn fatal(err: &anyhow::Error) -> ! {
    eprintln!("{err:?}");
    eprintln!("{err:#?}");
    std::process::exit(1);
}

// spanpaper — single-MP4 video wallpaper spanning across stacked Wayland monitors.
//
// Entry point: sets up tracing, parses the CLI, dispatches to subcommands.

mod cli;
mod config;
mod daemon;
mod ipc;
mod media;
mod outputs;
mod workers;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,spanpaper=debug"));
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(filter)
        .init();

    cli::dispatch(cli::Cli::parse())
}

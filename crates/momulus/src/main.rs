//! The momulus command-line interface.

mod cli;
mod commands;
mod logging;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Commands};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // A .env next to the working directory is loaded the same way docker compose
    // does it, so a local run needs no `source .env`. Variables already present
    // in the environment win.
    let dotenv = dotenvy::dotenv();

    let cli = Cli::parse();
    logging::init(cli.log_format, &cli.log_level)?;
    match dotenv {
        Ok(path) => tracing::debug!(path = %path.display(), "loaded .env"),
        Err(err) if err.not_found() => {}
        Err(err) => tracing::warn!(error = %err, "could not read .env"),
    }

    match &cli.command {
        Commands::Serve { dry_run } => commands::serve::run(&cli, *dry_run).await,
        Commands::Run(args) => commands::run::run(&cli, args).await,
        Commands::Skills { command } => commands::skills::run(&cli, command),
    }
}

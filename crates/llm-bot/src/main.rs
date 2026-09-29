//! CLI сервиса llm-bot.

mod cli;
mod commands;
mod logging;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Commands};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    logging::init(cli.log_format, &cli.log_level)?;

    match &cli.command {
        Commands::Serve { .. } => {
            anyhow::bail!("`serve` пока не реализован (этап 7)");
        }
        Commands::Run(args) => commands::run::run(&cli, args).await,
        Commands::Skills { command } => commands::skills::run(&cli, command),
    }
}

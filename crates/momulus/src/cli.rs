//! Command-line argument definitions.

use std::path::PathBuf;

use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "momulus",
    version,
    about = "An orchestrator for LLM skills on pull requests"
)]
pub struct Cli {
    /// Path to the config file.
    #[arg(
        long,
        short,
        global = true,
        default_value = "config.toml",
        env = "MOMULUS_CONFIG"
    )]
    pub config: PathBuf,

    /// Skills directory; overrides the value from the config.
    #[arg(long, global = true)]
    pub skills_dir: Option<PathBuf>,

    /// Log format.
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Text)]
    pub log_format: LogFormat,

    /// Log level (RUST_LOG takes precedence).
    #[arg(long, global = true, default_value = "info")]
    pub log_level: String,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// The main mode: poll the platform and execute commands.
    Serve {
        /// Do everything, publish nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Run a single skill on a local directory, without any platform.
    Run(RunArgs),

    /// Work with the skill registry.
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
}

#[derive(Debug, ClapArgs)]
pub struct RunArgs {
    /// Local git repository to run the skill against.
    #[arg(long)]
    pub repo_path: PathBuf,

    /// Skill name from the registry.
    #[arg(long)]
    pub skill: String,

    /// Files to treat as changed (comma-separated). Empty means everything matching the skill filter.
    #[arg(long, value_delimiter = ',')]
    pub files: Vec<String>,

    /// Skill argument as k=v; may be repeated.
    #[arg(long = "arg", value_parser = parse_kv)]
    pub args: Vec<(String, String)>,

    /// Where to put the artifacts (./out by default).
    #[arg(long, default_value = "out")]
    pub out: PathBuf,

    /// Take ready-made artifacts from a directory instead of using docker (for tests).
    #[arg(long, hide = true)]
    pub fake_runner: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum SkillsCommand {
    /// List the available skills.
    List,
    /// Validate the skill contracts.
    Validate,
}

fn parse_kv(raw: &str) -> Result<(String, String), String> {
    let (key, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("expected k=v, got {raw:?}"))?;
    if key.is_empty() {
        return Err(format!("empty argument name in {raw:?}"));
    }
    Ok((key.to_string(), value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_serve_with_dry_run() {
        let cli = Cli::parse_from(["momulus", "serve", "--dry-run"]);
        assert!(matches!(cli.command, Commands::Serve { dry_run: true }));
    }

    #[test]
    fn parses_run_with_files_and_args() {
        let cli = Cli::parse_from([
            "momulus",
            "run",
            "--repo-path",
            "/tmp/repo",
            "--skill",
            "translate",
            "--files",
            "a.mdx,b.mdx",
            "--arg",
            "lang=en",
        ]);
        let Commands::Run(args) = cli.command else {
            panic!("expected run");
        };
        assert_eq!(args.files, vec!["a.mdx", "b.mdx"]);
        assert_eq!(args.args, vec![("lang".to_string(), "en".to_string())]);
    }

    #[test]
    fn rejects_bad_arg_syntax() {
        assert!(
            Cli::try_parse_from([
                "momulus",
                "run",
                "--repo-path",
                ".",
                "--skill",
                "x",
                "--arg",
                "lang"
            ])
            .is_err()
        );
    }
}

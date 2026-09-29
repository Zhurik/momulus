//! `momulus skills list|validate`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use momulus_core::Config;
use momulus_skills::Registry;

use crate::cli::{Cli, SkillsCommand};

/// Where to look for skills: the flag, then the config (if any), then the default.
pub fn resolve_skills_dir(cli: &Cli) -> Result<PathBuf> {
    if let Some(dir) = &cli.skills_dir {
        return Ok(dir.clone());
    }
    if cli.config.exists() {
        let config = Config::load(&cli.config)
            .with_context(|| format!("config {}", cli.config.display()))?;
        return Ok(config.skills_dir);
    }
    Ok(PathBuf::from("skills"))
}

/// The config from a file, or sensible defaults for a local run when there is none.
pub fn load_config_or_default(cli: &Cli) -> Result<Config> {
    if cli.config.exists() {
        return Config::load(&cli.config)
            .with_context(|| format!("config {}", cli.config.display()));
    }
    Ok(Config::from_toml("allowed_users = [\"local\"]").expect("the defaults are valid"))
}

pub fn run(cli: &Cli, command: &SkillsCommand) -> Result<()> {
    let dir = resolve_skills_dir(cli)?;
    match command {
        SkillsCommand::List => list(&dir),
        SkillsCommand::Validate => validate(&dir),
    }
}

fn list(dir: &Path) -> Result<()> {
    let registry = Registry::load(dir).map_err(anyhow::Error::from)?;
    if registry.is_empty() {
        println!("no skills in {}", dir.display());
        return Ok(());
    }

    let rows: Vec<[String; 5]> = registry
        .iter()
        .map(|skill| {
            [
                skill.name.clone(),
                skill.contract.mode.to_string(),
                if skill.contract.args.is_empty() {
                    "-".to_string()
                } else {
                    skill.contract.args.join(",")
                },
                if skill.contract.files.is_empty() {
                    "*".to_string()
                } else {
                    skill.contract.files.join(",")
                },
                skill.doc.description.clone(),
            ]
        })
        .collect();

    let headers = ["NAME", "MODE", "ARGS", "FILES", "DESCRIPTION"];
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }

    print_row(&headers.map(String::from), &widths);
    for row in &rows {
        print_row(row, &widths);
    }
    Ok(())
}

fn print_row(row: &[String; 5], widths: &[usize]) {
    let mut line = String::new();
    for (i, cell) in row.iter().enumerate() {
        if i + 1 == row.len() {
            line.push_str(cell);
        } else {
            let pad = widths[i].saturating_sub(cell.chars().count()) + 2;
            line.push_str(cell);
            line.push_str(&" ".repeat(pad));
        }
    }
    println!("{}", line.trim_end());
}

fn validate(dir: &Path) -> Result<()> {
    let report = Registry::load_report(dir).map_err(anyhow::Error::from)?;
    for skill in report.registry.iter() {
        println!("ok   {} ({})", skill.name, skill.contract.mode);
    }
    for error in &report.errors {
        eprintln!("FAIL {error}");
    }
    if !report.errors.is_empty() {
        bail!(
            "invalid skills: {} of {}",
            report.errors.len(),
            report.errors.len() + report.registry.len()
        );
    }
    println!("\nall contracts are valid: {}", report.registry.len());
    Ok(())
}

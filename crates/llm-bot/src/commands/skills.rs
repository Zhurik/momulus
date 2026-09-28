//! `llm-bot skills list|validate`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use llm_bot_core::Config;
use llm_bot_skills::Registry;

use crate::cli::{Cli, SkillsCommand};

/// Где искать скиллы: флаг, потом конфиг (если он есть), потом дефолт.
pub fn resolve_skills_dir(cli: &Cli) -> Result<PathBuf> {
    if let Some(dir) = &cli.skills_dir {
        return Ok(dir.clone());
    }
    if cli.config.exists() {
        let config = Config::load(&cli.config)
            .with_context(|| format!("конфиг {}", cli.config.display()))?;
        return Ok(config.skills_dir);
    }
    Ok(PathBuf::from("skills"))
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
        println!("в {} нет скиллов", dir.display());
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
            "невалидных скиллов: {} (из {})",
            report.errors.len(),
            report.errors.len() + report.registry.len()
        );
    }
    println!("\nвсе контракты валидны: {}", report.registry.len());
    Ok(())
}

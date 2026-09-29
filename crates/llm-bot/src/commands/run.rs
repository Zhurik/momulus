//! `llm-bot run` — прогон одного скилла на локальной папке, без платформы.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use llm_bot_core::{Args as CommandArgs, Config, JobId, Mount, RunSpec, Runner, Secrets};
use llm_bot_pipeline::FakeRunner;
use llm_bot_pipeline::prompt::{FINDINGS_FILE, Origin, PromptContext, SUMMARY_FILE};
use llm_bot_runner_docker::{DockerRunner, models_json};
use llm_bot_skills::{Mode, Registry, Skill};

use crate::cli::{Cli, RunArgs};
use crate::commands::skills::{load_config_or_default, resolve_skills_dir};

/// Каталоги, которые не имеет смысла обходить в поисках файлов скилла.
const SKIP_DIRS: [&str; 6] = [".git", "node_modules", "target", ".next", "dist", ".venv"];

pub async fn run(cli: &Cli, args: &RunArgs) -> Result<()> {
    let config = load_config_or_default(cli)?;
    let skills_dir = absolute(&resolve_skills_dir(cli)?)?;
    let registry = Registry::load(&skills_dir).map_err(anyhow::Error::from)?;
    let skill = registry.get(&args.skill).map_err(anyhow::Error::from)?;

    let repo_path = absolute(&args.repo_path)?;
    if !repo_path.is_dir() {
        bail!("{} — не каталог", repo_path.display());
    }

    let files = select_files(&repo_path, skill, &args.files, &config)?;
    if files.is_empty() {
        println!(
            "нечего делать: под фильтр скилла {:?} не попал ни один файл",
            skill.contract.files
        );
        return Ok(());
    }

    let named: BTreeMap<String, String> = args.args.iter().cloned().collect();
    let command_args = CommandArgs {
        positional: Vec::new(),
        named,
    };
    let resolved = skill
        .contract
        .resolve_args(&skill.name, &command_args)
        .map_err(anyhow::Error::from)?;

    let prompt = PromptContext {
        skill,
        files: &files,
        args: &resolved,
        origin: Origin::Local {
            path: repo_path.display().to_string(),
        },
    }
    .render();

    let out_dir = absolute(&args.out)?;
    std::fs::create_dir_all(&out_dir)?;

    let secrets = Secrets::from_env()?;
    let (runner, spec) = build_runner(
        args,
        &config,
        &secrets,
        skill,
        &repo_path,
        &skills_dir,
        &out_dir,
        prompt,
    )?;

    println!(
        "скилл {} ({}), файлов: {}, каталог артефактов: {}",
        skill.name,
        skill.contract.mode,
        files.len(),
        out_dir.display()
    );

    let result = runner.run(spec).await?;

    let log_path = out_dir.join("pi.log");
    std::fs::write(&log_path, result.combined_log())?;

    println!("\n--- вывод pi ---\n{}", result.stdout.trim_end());
    if !result.stderr.trim().is_empty() {
        eprintln!("--- stderr pi ---\n{}", result.stderr.trim_end());
    }
    println!(
        "\nкод возврата: {}{}\nлог: {}",
        result.exit_code,
        if result.timed_out {
            " (таймаут)"
        } else {
            ""
        },
        log_path.display()
    );

    if result.timed_out {
        bail!("скилл не уложился в таймаут {:?}", skill.contract.timeout);
    }
    if !result.is_success() {
        bail!("pi завершился с кодом {}", result.exit_code);
    }

    print_artifacts(skill, &out_dir)?;
    Ok(())
}

/// Собирает runner и спецификацию запуска.
#[allow(clippy::too_many_arguments)]
fn build_runner(
    args: &RunArgs,
    config: &Config,
    secrets: &Secrets,
    skill: &Skill,
    repo_path: &Path,
    skills_dir: &Path,
    out_dir: &Path,
    prompt: String,
) -> Result<(Box<dyn Runner>, RunSpec)> {
    let model = if skill.contract.model.is_empty() {
        config.llm.default_model.clone()
    } else {
        skill.contract.model.clone()
    };

    let mut env = Vec::new();
    let mut agent_config = None;
    let runner: Box<dyn Runner> = match &args.fake_runner {
        Some(dir) => Box::new(FakeRunner::from_dir(dir)?),
        None => {
            let key = secrets
                .require_llm_api_key()
                .context("для реального прогона нужен ключ провайдера")?;
            env.push((config.llm.api_key_env(), key.to_string()));
            if let Some(base_url) = &secrets.llm_base_url {
                agent_config = Some(models_json(
                    &config.llm.default_provider,
                    base_url,
                    &config.llm.api_key_env(),
                )?);
            }
            let redactor = secrets.redactor();
            Box::new(DockerRunner::connect(
                config.docker.host.as_deref(),
                config.docker.user.clone(),
                redactor,
            )?)
        }
    };

    let spec = RunSpec {
        job_id: JobId::new(),
        skill: skill.name.clone(),
        workdir: repo_path.to_path_buf(),
        skills_dir: skills_dir.to_path_buf(),
        out_dir: out_dir.to_path_buf(),
        mount: match skill.contract.mode {
            Mode::Review => Mount::ReadOnly,
            Mode::Patch => Mount::ReadWrite,
        },
        prompt,
        tools: skill.contract.tools.clone(),
        provider: config.llm.default_provider.clone(),
        model: Some(model),
        timeout: skill.contract.timeout,
        image: config.docker.runner_image.clone(),
        cpu_limit: config.docker.cpu,
        memory_limit_mb: config.docker.memory_mb,
        env,
        agent_config,
    };
    Ok((runner, spec))
}

/// Показывает, что скилл положил в артефакты.
fn print_artifacts(skill: &Skill, out_dir: &Path) -> Result<()> {
    match skill.contract.mode {
        Mode::Review => {
            let path = out_dir.join(FINDINGS_FILE);
            if !path.exists() {
                bail!("скилл не записал {}", path.display());
            }
            println!(
                "\n--- {} ---\n{}",
                path.display(),
                std::fs::read_to_string(&path)?
            );
        }
        Mode::Patch => {
            let summary = out_dir.join(SUMMARY_FILE);
            if summary.exists() {
                println!(
                    "\n--- {} ---\n{}",
                    summary.display(),
                    std::fs::read_to_string(&summary)?
                );
            }
            println!("изменения остались в рабочей копии — посмотри `git status`/`git diff`");
        }
    }
    Ok(())
}

/// Файлы, с которыми будет работать скилл.
fn select_files(
    root: &Path,
    skill: &Skill,
    requested: &[String],
    config: &Config,
) -> Result<Vec<String>> {
    let candidates: Vec<String> = if requested.is_empty() {
        let mut found = Vec::new();
        walk(root, root, &mut found)?;
        found.sort();
        found
    } else {
        for file in requested {
            if !root.join(file).exists() {
                bail!("файла {file} нет в {}", root.display());
            }
        }
        requested.to_vec()
    };

    let selected = skill
        .contract
        .select_files(candidates.iter().map(String::as_str))
        .map_err(anyhow::Error::from)?;

    if selected.len() > config.limits.max_changed_files {
        bail!(
            "файлов слишком много: {} при лимите {} (сузь список через --files)",
            selected.len(),
            config.limits.max_changed_files
        );
    }
    Ok(selected)
}

/// Рекурсивно собирает относительные пути файлов, пропуская служебные каталоги.
fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if path.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().to_string());
        }
    }
    Ok(())
}

/// Docker принимает только абсолютные пути монтирования.
fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir()?;
    Ok(cwd.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_skips_service_directories() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::create_dir_all(tmp.path().join("posts")).unwrap();
        std::fs::write(tmp.path().join(".git/config"), "x").unwrap();
        std::fs::write(tmp.path().join("posts/a.mdx"), "x").unwrap();
        std::fs::write(tmp.path().join("README.md"), "x").unwrap();

        let mut found = Vec::new();
        walk(tmp.path(), tmp.path(), &mut found).unwrap();
        found.sort();
        assert_eq!(found, vec!["README.md", "posts/a.mdx"]);
    }
}

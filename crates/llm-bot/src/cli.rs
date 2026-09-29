//! Описание аргументов командной строки.

use std::path::PathBuf;

use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "llm-bot",
    version,
    about = "Оркестратор LLM-скиллов для pull request'ов"
)]
pub struct Cli {
    /// Путь к конфигу.
    #[arg(
        long,
        short,
        global = true,
        default_value = "config.toml",
        env = "LLM_BOT_CONFIG"
    )]
    pub config: PathBuf,

    /// Каталог со скиллами; перебивает значение из конфига.
    #[arg(long, global = true)]
    pub skills_dir: Option<PathBuf>,

    /// Формат логов.
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Text)]
    pub log_format: LogFormat,

    /// Уровень логов (перебивается RUST_LOG).
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
    /// Основной режим: опрос платформы и выполнение команд.
    Serve {
        /// Всё выполнить, ничего не публиковать.
        #[arg(long)]
        dry_run: bool,
    },

    /// Прогон одного скилла на локальной папке, без платформы.
    Run(RunArgs),

    /// Работа с реестром скиллов.
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
}

#[derive(Debug, ClapArgs)]
pub struct RunArgs {
    /// Локальный git-репозиторий, на котором прогоняем скилл.
    #[arg(long)]
    pub repo_path: PathBuf,

    /// Имя скилла из реестра.
    #[arg(long)]
    pub skill: String,

    /// Файлы, которые считаем изменёнными (через запятую). Пусто — все под фильтром скилла.
    #[arg(long, value_delimiter = ',')]
    pub files: Vec<String>,

    /// Аргумент скилла в форме k=v; можно повторять.
    #[arg(long = "arg", value_parser = parse_kv)]
    pub args: Vec<(String, String)>,

    /// Куда класть артефакты (по умолчанию ./out).
    #[arg(long, default_value = "out")]
    pub out: PathBuf,

    /// Вместо docker взять готовые артефакты из каталога (для тестов).
    #[arg(long, hide = true)]
    pub fake_runner: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum SkillsCommand {
    /// Показать доступные скиллы.
    List,
    /// Проверить контракты скиллов.
    Validate,
}

fn parse_kv(raw: &str) -> Result<(String, String), String> {
    let (key, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("ожидается k=v, получено {raw:?}"))?;
    if key.is_empty() {
        return Err(format!("пустое имя аргумента в {raw:?}"));
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
        let cli = Cli::parse_from(["llm-bot", "serve", "--dry-run"]);
        assert!(matches!(cli.command, Commands::Serve { dry_run: true }));
    }

    #[test]
    fn parses_run_with_files_and_args() {
        let cli = Cli::parse_from([
            "llm-bot",
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
                "llm-bot",
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

//! Разбор и валидация `skill.toml` — контракта скилла для оркестратора.

use std::collections::BTreeMap;
use std::time::Duration;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::error::{SkillError, SkillResult};

/// Режим работы скилла.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Модель ищет замечания, мы публикуем ревью.
    Review,
    /// Модель правит файлы, мы коммитим и открываем PR.
    Patch,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Review => "review",
            Mode::Patch => "patch",
        }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Содержимое `skill.toml` как оно записано в файле.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillContract {
    pub mode: Mode,

    /// Инструменты, которые получит pi (`--tools`).
    #[serde(default)]
    pub tools: Vec<String>,

    /// Глобы по изменённым файлам PR; пусто — берём все файлы.
    #[serde(default)]
    pub files: Vec<String>,

    /// Имена аргументов команды; позиционные args мапятся в этом порядке.
    #[serde(default)]
    pub args: Vec<String>,

    /// Таймаут одного прогона.
    #[serde(with = "humantime_serde", default = "default_timeout")]
    pub timeout: Duration,

    /// Модель; пусто — значение из конфига.
    #[serde(default)]
    pub model: String,

    /// Максимум inline-комментариев (только review).
    #[serde(default)]
    pub max_comments: Option<usize>,

    /// Шаблон имени ветки (только patch).
    #[serde(default)]
    pub branch: Option<String>,

    /// Произвольные переменные контракта, попадают в промпт как есть.
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
}

fn default_timeout() -> Duration {
    Duration::from_secs(600)
}

/// Значение `max_comments` по умолчанию.
pub const DEFAULT_MAX_COMMENTS: usize = 30;
/// Шаблон ветки по умолчанию.
pub const DEFAULT_BRANCH_TEMPLATE: &str = "llm/{skill}-{pr}";

impl SkillContract {
    /// Разбирает и валидирует контракт.
    pub fn parse(skill: &str, text: &str) -> SkillResult<SkillContract> {
        let contract: SkillContract = toml::from_str(text).map_err(|e| SkillError::Contract {
            skill: skill.to_string(),
            message: compact_toml_error(&e.to_string()),
        })?;
        contract.validate(skill)?;
        Ok(contract)
    }

    fn validate(&self, skill: &str) -> SkillResult<()> {
        let fail = |message: String| SkillError::Contract {
            skill: skill.to_string(),
            message,
        };

        if self.tools.is_empty() && self.mode == Mode::Patch {
            return Err(fail(
                "mode = \"patch\" требует непустой tools: агент должен уметь править файлы".into(),
            ));
        }
        for tool in &self.tools {
            if tool.trim().is_empty() {
                return Err(fail("tools содержит пустую строку".into()));
            }
        }
        if self.timeout.is_zero() {
            return Err(fail("timeout должен быть больше нуля".into()));
        }
        if self.timeout > Duration::from_secs(3600) {
            return Err(fail("timeout больше часа — так не делаем".into()));
        }
        // Глобы проверяем сразу, чтобы ошибка была видна при старте, а не в джобе.
        self.file_filter()
            .map_err(|e| fail(format!("невалидный glob в files: {e}")))?;

        for arg in &self.args {
            if arg.is_empty() || !arg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(fail(format!(
                    "имя аргумента {arg:?} должно состоять из букв, цифр и _"
                )));
            }
        }

        match self.mode {
            Mode::Review => {
                if self.branch.is_some() {
                    return Err(fail(
                        "branch имеет смысл только при mode = \"patch\"".into(),
                    ));
                }
                if self.max_comments == Some(0) {
                    return Err(fail("max_comments должен быть больше нуля".into()));
                }
            }
            Mode::Patch => {
                if self.max_comments.is_some() {
                    return Err(fail(
                        "max_comments имеет смысл только при mode = \"review\"".into(),
                    ));
                }
                if let Some(branch) = &self.branch {
                    if branch.trim().is_empty() {
                        return Err(fail("branch не может быть пустым".into()));
                    }
                    for placeholder in placeholders(branch) {
                        let known = placeholder == "skill"
                            || placeholder == "pr"
                            || self.args.iter().any(|a| a == placeholder);
                        if !known {
                            return Err(fail(format!(
                                "branch использует неизвестный placeholder {{{placeholder}}}"
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Компилирует глобы фильтра файлов; `None` — фильтра нет.
    pub fn file_filter(&self) -> Result<Option<GlobSet>, globset::Error> {
        if self.files.is_empty() {
            return Ok(None);
        }
        let mut builder = GlobSetBuilder::new();
        for pattern in &self.files {
            builder.add(Glob::new(pattern)?);
        }
        builder.build().map(Some)
    }

    /// Отбирает из списка изменённых файлов те, что подходят скиллу.
    pub fn select_files<'a, I>(&self, files: I) -> SkillResult<Vec<String>>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let filter = self
            .file_filter()
            .map_err(|e| SkillError::Internal(format!("невалидный glob: {e}")))?;
        Ok(files
            .into_iter()
            .filter(|path| filter.as_ref().is_none_or(|set| set.is_match(path)))
            .map(str::to_string)
            .collect())
    }

    pub fn max_comments(&self) -> usize {
        self.max_comments.unwrap_or(DEFAULT_MAX_COMMENTS)
    }

    pub fn branch_template(&self) -> &str {
        self.branch.as_deref().unwrap_or(DEFAULT_BRANCH_TEMPLATE)
    }

    /// Имя ветки для патча: подставляет {skill}, {pr} и аргументы скилла.
    pub fn branch_name(&self, skill: &str, pr: u64, args: &BTreeMap<String, String>) -> String {
        let mut out = self
            .branch_template()
            .replace("{skill}", skill)
            .replace("{pr}", &pr.to_string());
        for (key, value) in args {
            out = out.replace(&format!("{{{key}}}"), &slugify(value));
        }
        out
    }

    /// Сопоставляет аргументы команды с именами из контракта.
    ///
    /// Позиционные значения раздаются по порядку `args`, именованные должны
    /// совпадать с объявленными.
    pub fn resolve_args(
        &self,
        skill: &str,
        args: &llm_bot_core::Args,
    ) -> SkillResult<BTreeMap<String, String>> {
        let fail = |message: String| SkillError::Args {
            skill: skill.to_string(),
            message,
        };

        let mut resolved: BTreeMap<String, String> = BTreeMap::new();
        for (key, value) in &args.named {
            if !self.args.contains(key) {
                return Err(fail(format!(
                    "неизвестный аргумент {key:?}; скилл принимает: {}",
                    self.args_help()
                )));
            }
            resolved.insert(key.clone(), value.clone());
        }

        let free: Vec<String> = self
            .args
            .iter()
            .filter(|name| !resolved.contains_key(*name))
            .cloned()
            .collect();
        let mut free = free.into_iter();
        for value in &args.positional {
            match free.next() {
                Some(name) => {
                    resolved.insert(name, value.clone());
                }
                None => {
                    return Err(fail(format!(
                        "лишний аргумент {value:?}; скилл принимает: {}",
                        self.args_help()
                    )));
                }
            }
        }

        let missing: Vec<&str> = self
            .args
            .iter()
            .filter(|name| !resolved.contains_key(*name))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            return Err(fail(format!(
                "не хватает аргументов: {}",
                missing.join(", ")
            )));
        }
        Ok(resolved)
    }

    fn args_help(&self) -> String {
        if self.args.is_empty() {
            "нет аргументов".to_string()
        } else {
            self.args.join(", ")
        }
    }
}

/// Сжимает многострочную ошибку toml до "описание (позиция)".
/// Имена в фигурных скобках из шаблона.
fn placeholders(template: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                out.push(&after[..close]);
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    out
}

/// Приводит значение аргумента к безопасному для имени ветки виду.
fn slugify(value: &str) -> String {
    let slug: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    slug.trim_matches('-').to_string()
}

fn compact_toml_error(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let position = lines
        .iter()
        .find(|line| line.starts_with("TOML parse error"))
        .map(|line| line.trim_start_matches("TOML parse error").trim());
    let reason = lines
        .iter()
        .rev()
        .find(|line| {
            !line.starts_with("TOML parse error")
                && !line.trim_start().starts_with('|')
                && !line.contains(" | ")
        })
        .copied();

    match (reason, position) {
        (Some(reason), Some(position)) if !position.is_empty() => {
            format!("{reason} ({position})")
        }
        (Some(reason), _) => reason.to_string(),
        (None, _) => text.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_bot_core::Command;

    const REVIEW: &str = r#"
        mode = "review"
        tools = ["read", "grep"]
        files = ["**/*.md", "**/*.mdx"]
        timeout = "10m"
        max_comments = 5
    "#;

    #[test]
    fn parses_review_contract() {
        let c = SkillContract::parse("proofread", REVIEW).unwrap();
        assert_eq!(c.mode, Mode::Review);
        assert_eq!(c.tools, vec!["read", "grep"]);
        assert_eq!(c.timeout, Duration::from_secs(600));
        assert_eq!(c.max_comments(), 5);
        assert!(c.model.is_empty());
    }

    #[test]
    fn applies_defaults() {
        let c = SkillContract::parse("x", r#"mode = "review""#).unwrap();
        assert_eq!(c.timeout, Duration::from_secs(600));
        assert_eq!(c.max_comments(), DEFAULT_MAX_COMMENTS);
        assert!(c.files.is_empty());
        assert!(c.args.is_empty());
    }

    #[test]
    fn rejects_unknown_fields() {
        let err = SkillContract::parse("x", "mode = \"review\"\nfile = [\"*.md\"]").unwrap_err();
        assert!(err.to_string().contains("file"), "{err}");
    }

    #[test]
    fn rejects_unknown_mode() {
        let err = SkillContract::parse("x", r#"mode = "translate""#).unwrap_err();
        assert!(
            err.to_string().contains("expected `review` or `patch`"),
            "{err}"
        );
    }

    #[test]
    fn rejects_patch_without_tools() {
        let err = SkillContract::parse("t", r#"mode = "patch""#).unwrap_err();
        assert!(err.to_string().contains("tools"), "{err}");
    }

    #[test]
    fn rejects_invalid_glob() {
        let err =
            SkillContract::parse("x", "mode = \"review\"\nfiles = [\"**/*.{md\"]").unwrap_err();
        assert!(err.to_string().contains("glob"), "{err}");
    }

    #[test]
    fn rejects_branch_in_review_mode() {
        let err = SkillContract::parse("x", "mode = \"review\"\nbranch = \"llm/x\"").unwrap_err();
        assert!(err.to_string().contains("branch"), "{err}");
    }

    #[test]
    fn rejects_max_comments_in_patch_mode() {
        let err = SkillContract::parse(
            "x",
            "mode = \"patch\"\ntools = [\"edit\"]\nmax_comments = 3",
        )
        .unwrap_err();
        assert!(err.to_string().contains("max_comments"), "{err}");
    }

    #[test]
    fn rejects_absurd_timeout() {
        let err = SkillContract::parse("x", "mode = \"review\"\ntimeout = \"3h\"").unwrap_err();
        assert!(err.to_string().contains("timeout"), "{err}");
        let err = SkillContract::parse("x", "mode = \"review\"\ntimeout = \"0s\"").unwrap_err();
        assert!(err.to_string().contains("timeout"), "{err}");
    }

    #[test]
    fn rejects_bad_arg_names() {
        let err = SkillContract::parse("x", "mode = \"review\"\nargs = [\"la ng\"]").unwrap_err();
        assert!(err.to_string().contains("аргумент"), "{err}");
    }

    #[test]
    fn selects_files_by_globs() {
        let c = SkillContract::parse("proofread", REVIEW).unwrap();
        let selected = c
            .select_files(["posts/a.mdx", "src/main.rs", "README.md", "deep/b/c.md"])
            .unwrap();
        assert_eq!(selected, vec!["posts/a.mdx", "README.md", "deep/b/c.md"]);
    }

    #[test]
    fn empty_filter_takes_everything() {
        let c = SkillContract::parse("x", r#"mode = "review""#).unwrap();
        let selected = c.select_files(["a.rs", "b.md"]).unwrap();
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn branch_name_expands_template() {
        let c = SkillContract::parse(
            "translate",
            "mode = \"patch\"\ntools = [\"edit\"]\nbranch = \"llm/{skill}-{pr}\"",
        )
        .unwrap();
        assert_eq!(
            c.branch_name("translate", 17, &BTreeMap::new()),
            "llm/translate-17"
        );
    }

    #[test]
    fn default_branch_template_is_used() {
        let c = SkillContract::parse("t", "mode = \"patch\"\ntools = [\"edit\"]").unwrap();
        assert_eq!(c.branch_name("t", 3, &BTreeMap::new()), "llm/t-3");
    }

    #[test]
    fn resolves_positional_args() {
        let c = SkillContract::parse(
            "translate",
            "mode = \"patch\"\ntools = [\"edit\"]\nargs = [\"lang\"]",
        )
        .unwrap();
        let cmd = Command::parse("/llm translate en").unwrap();
        let resolved = c.resolve_args("translate", &cmd.args).unwrap();
        assert_eq!(resolved.get("lang").map(String::as_str), Some("en"));
    }

    #[test]
    fn resolves_named_args() {
        let c = SkillContract::parse(
            "translate",
            "mode = \"patch\"\ntools = [\"edit\"]\nargs = [\"lang\"]",
        )
        .unwrap();
        let cmd = Command::parse("/llm translate lang=de").unwrap();
        let resolved = c.resolve_args("translate", &cmd.args).unwrap();
        assert_eq!(resolved.get("lang").map(String::as_str), Some("de"));
    }

    #[test]
    fn reports_missing_args() {
        let c = SkillContract::parse(
            "translate",
            "mode = \"patch\"\ntools = [\"edit\"]\nargs = [\"lang\"]",
        )
        .unwrap();
        let cmd = Command::parse("/llm translate").unwrap();
        let err = c.resolve_args("translate", &cmd.args).unwrap_err();
        assert!(err.to_string().contains("lang"), "{err}");
    }

    #[test]
    fn reports_unknown_and_extra_args() {
        let c = SkillContract::parse(
            "translate",
            "mode = \"patch\"\ntools = [\"edit\"]\nargs = [\"lang\"]",
        )
        .unwrap();
        let cmd = Command::parse("/llm translate style=formal").unwrap();
        let err = c.resolve_args("translate", &cmd.args).unwrap_err();
        assert!(err.to_string().contains("style"), "{err}");

        let cmd = Command::parse("/llm translate en de").unwrap();
        let err = c.resolve_args("translate", &cmd.args).unwrap_err();
        assert!(err.to_string().contains("лишний"), "{err}");
    }

    #[test]
    fn branch_name_expands_skill_args() {
        let c = SkillContract::parse(
            "translate",
            "mode = \"patch\"\ntools = [\"edit\"]\nargs = [\"lang\"]\nbranch = \"llm/{skill}-{lang}-{pr}\"",
        )
        .unwrap();
        let args = BTreeMap::from([("lang".to_string(), "en GB".to_string())]);
        assert_eq!(
            c.branch_name("translate", 7, &args),
            "llm/translate-en-gb-7"
        );
    }

    #[test]
    fn rejects_unknown_branch_placeholder() {
        let err = SkillContract::parse(
            "t",
            "mode = \"patch\"\ntools = [\"edit\"]\nbranch = \"llm/{lang}\"",
        )
        .unwrap_err();
        assert!(err.to_string().contains("placeholder"), "{err}");
    }

    #[test]
    fn vars_are_free_form() {
        let c = SkillContract::parse(
            "translate",
            "mode = \"patch\"\ntools = [\"edit\"]\n[vars]\nnaming = \"{stem}.{lang}{ext}\"",
        )
        .unwrap();
        assert_eq!(
            c.vars.get("naming").map(String::as_str),
            Some("{stem}.{lang}{ext}")
        );
    }
}

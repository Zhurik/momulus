//! Parsing and validation of `skill.toml` — the skill contract for the orchestrator.

use std::collections::BTreeMap;
use std::time::Duration;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::error::{SkillError, SkillResult};

/// Skill mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The model looks for findings, we publish a review.
    Review,
    /// The model edits files, we commit and open a PR.
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

/// The contents of `skill.toml` exactly as written in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillContract {
    pub mode: Mode,

    /// Tools handed to pi (`--tools`).
    #[serde(default)]
    pub tools: Vec<String>,

    /// Globs over the changed PR files; empty means every file.
    #[serde(default)]
    pub files: Vec<String>,

    /// Command argument names; positional args map onto them in this order.
    #[serde(default)]
    pub args: Vec<String>,

    /// Timeout for a single run.
    #[serde(with = "humantime_serde", default = "default_timeout")]
    pub timeout: Duration,

    /// Model; empty means the value from the config.
    #[serde(default)]
    pub model: String,

    /// Maximum number of inline comments (review only).
    #[serde(default)]
    pub max_comments: Option<usize>,

    /// Branch name template (patch only).
    #[serde(default)]
    pub branch: Option<String>,

    /// Free-form contract variables, passed into the prompt as is.
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
}

fn default_timeout() -> Duration {
    Duration::from_secs(600)
}

/// Default value of `max_comments`.
pub const DEFAULT_MAX_COMMENTS: usize = 30;
/// Default branch template.
pub const DEFAULT_BRANCH_TEMPLATE: &str = "llm/{skill}-{pr}";

impl SkillContract {
    /// Parses and validates the contract.
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
                "mode = \"patch\" requires a non-empty tools list: the agent must be able to edit files".into(),
            ));
        }
        // Without write the agent cannot produce an artifact: findings.json in review
        // mode, edited files in patch mode. A review skill's working copy is protected
        // by mounting /work read-only, not by withholding the write tool.
        if !self.tools.is_empty() && !self.tools.iter().any(|t| t == "write") {
            let what = match self.mode {
                Mode::Review => "write /out/findings.json",
                Mode::Patch => "create files",
            };
            return Err(fail(format!(
                "tools has no \"write\", and without it the agent cannot {what}"
            )));
        }
        for tool in &self.tools {
            if tool.trim().is_empty() {
                return Err(fail("tools contains an empty string".into()));
            }
        }
        if self.timeout.is_zero() {
            return Err(fail("timeout must be greater than zero".into()));
        }
        if self.timeout > Duration::from_secs(3600) {
            return Err(fail("timeout over an hour is not allowed".into()));
        }
        // Globs are checked eagerly so the error shows up at startup, not mid-job.
        self.file_filter()
            .map_err(|e| fail(format!("invalid glob in files: {e}")))?;

        for arg in &self.args {
            if arg.is_empty() || !arg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(fail(format!(
                    "argument name {arg:?} must consist of letters, digits and _"
                )));
            }
        }

        match self.mode {
            Mode::Review => {
                if self.branch.is_some() {
                    return Err(fail("branch only makes sense with mode = \"patch\"".into()));
                }
                if self.max_comments == Some(0) {
                    return Err(fail("max_comments must be greater than zero".into()));
                }
            }
            Mode::Patch => {
                if self.max_comments.is_some() {
                    return Err(fail(
                        "max_comments only makes sense with mode = \"review\"".into(),
                    ));
                }
                if let Some(branch) = &self.branch {
                    if branch.trim().is_empty() {
                        return Err(fail("branch cannot be empty".into()));
                    }
                    for placeholder in placeholders(branch) {
                        let known = placeholder == "skill"
                            || placeholder == "pr"
                            || self.args.iter().any(|a| a == placeholder);
                        if !known {
                            return Err(fail(format!(
                                "branch uses an unknown placeholder {{{placeholder}}}"
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Compiles the file filter globs; `None` means there is no filter.
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

    /// Selects the changed files that match the skill.
    pub fn select_files<'a, I>(&self, files: I) -> SkillResult<Vec<String>>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let filter = self
            .file_filter()
            .map_err(|e| SkillError::Internal(format!("invalid glob: {e}")))?;
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

    /// Branch name for a patch: substitutes {skill}, {pr} and the skill arguments.
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

    /// Maps command arguments onto the names declared in the contract.
    ///
    /// Positional values are handed out in `args` order; named ones must match
    /// a declared name.
    pub fn resolve_args(
        &self,
        skill: &str,
        args: &momulus_core::Args,
    ) -> SkillResult<BTreeMap<String, String>> {
        let fail = |message: String| SkillError::Args {
            skill: skill.to_string(),
            message,
        };

        let mut resolved: BTreeMap<String, String> = BTreeMap::new();
        for (key, value) in &args.named {
            if !self.args.contains(key) {
                return Err(fail(format!(
                    "unknown argument {key:?}; the skill accepts: {}",
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
                        "extra argument {value:?}; the skill accepts: {}",
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
            return Err(fail(format!("missing arguments: {}", missing.join(", "))));
        }
        Ok(resolved)
    }

    fn args_help(&self) -> String {
        if self.args.is_empty() {
            "no arguments".to_string()
        } else {
            self.args.join(", ")
        }
    }
}

/// Compresses a multi-line toml error into "reason (position)".
/// Names wrapped in braces inside a template.
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

/// Turns an argument value into something safe for a branch name.
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
    use momulus_core::Command;

    const REVIEW: &str = r#"
        mode = "review"
        tools = ["read", "grep", "write"]
        files = ["**/*.md", "**/*.mdx"]
        timeout = "10m"
        max_comments = 5
    "#;

    #[test]
    fn parses_review_contract() {
        let c = SkillContract::parse("proofread", REVIEW).unwrap();
        assert_eq!(c.mode, Mode::Review);
        assert_eq!(c.tools, vec!["read", "grep", "write"]);
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
            "mode = \"patch\"\ntools = [\"write\"]\nmax_comments = 3",
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
    fn rejects_tools_without_write() {
        let err = SkillContract::parse("x", "mode = \"review\"\ntools = [\"read\"]").unwrap_err();
        assert!(err.to_string().contains("write"), "{err}");
        assert!(err.to_string().contains("findings.json"), "{err}");
    }

    #[test]
    fn rejects_bad_arg_names() {
        let err = SkillContract::parse("x", "mode = \"review\"\nargs = [\"la ng\"]").unwrap_err();
        assert!(err.to_string().contains("argument name"), "{err}");
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
            "mode = \"patch\"\ntools = [\"write\"]\nbranch = \"llm/{skill}-{pr}\"",
        )
        .unwrap();
        assert_eq!(
            c.branch_name("translate", 17, &BTreeMap::new()),
            "llm/translate-17"
        );
    }

    #[test]
    fn default_branch_template_is_used() {
        let c = SkillContract::parse("t", "mode = \"patch\"\ntools = [\"write\"]").unwrap();
        assert_eq!(c.branch_name("t", 3, &BTreeMap::new()), "llm/t-3");
    }

    #[test]
    fn resolves_positional_args() {
        let c = SkillContract::parse(
            "translate",
            "mode = \"patch\"\ntools = [\"write\"]\nargs = [\"lang\"]",
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
            "mode = \"patch\"\ntools = [\"write\"]\nargs = [\"lang\"]",
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
            "mode = \"patch\"\ntools = [\"write\"]\nargs = [\"lang\"]",
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
            "mode = \"patch\"\ntools = [\"write\"]\nargs = [\"lang\"]",
        )
        .unwrap();
        let cmd = Command::parse("/llm translate style=formal").unwrap();
        let err = c.resolve_args("translate", &cmd.args).unwrap_err();
        assert!(err.to_string().contains("style"), "{err}");

        let cmd = Command::parse("/llm translate en de").unwrap();
        let err = c.resolve_args("translate", &cmd.args).unwrap_err();
        assert!(err.to_string().contains("extra argument"), "{err}");
    }

    #[test]
    fn branch_name_expands_skill_args() {
        let c = SkillContract::parse(
            "translate",
            "mode = \"patch\"\ntools = [\"write\"]\nargs = [\"lang\"]\nbranch = \"llm/{skill}-{lang}-{pr}\"",
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
            "mode = \"patch\"\ntools = [\"write\"]\nbranch = \"llm/{lang}\"",
        )
        .unwrap_err();
        assert!(err.to_string().contains("placeholder"), "{err}");
    }

    #[test]
    fn vars_are_free_form() {
        let c = SkillContract::parse(
            "translate",
            "mode = \"patch\"\ntools = [\"write\"]\n[vars]\nnaming = \"{stem}.{lang}{ext}\"",
        )
        .unwrap();
        assert_eq!(
            c.vars.get("naming").map(String::as_str),
            Some("{stem}.{lang}{ext}")
        );
    }
}

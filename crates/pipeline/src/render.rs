//! Тексты, которые бот пишет в PR. Платформенно-независимый markdown.

use momulus_core::{Finding, JobId};

use crate::validate::ReviewResult;

/// Общий контекст для подписи под сообщением.
#[derive(Debug, Clone)]
pub struct JobContext<'a> {
    pub job_id: JobId,
    pub skill: &'a str,
    /// Команда, как её написал человек.
    pub command: &'a str,
}

impl JobContext<'_> {
    /// Подпись: по ней находят логи джобы.
    fn footer(&self) -> String {
        format!(
            "\n<sub>скилл `{}` · job `{}`</sub>\n",
            self.skill,
            self.job_id.short()
        )
    }
}

/// Тело ревью: резюме модели плюс то, что не уложилось в inline-комментарии.
pub fn review_summary(result: &ReviewResult, ctx: &JobContext<'_>, files: usize) -> String {
    let mut out = format!(
        "**`{}`** — проверено файлов: {}, замечаний: {}.\n\n",
        ctx.command,
        files,
        result.inline.len() + result.out_of_diff.len() + result.unknown_path.len()
    );

    if !result.summary.trim().is_empty() {
        out.push_str(result.summary.trim());
        out.push_str("\n\n");
    }

    if !result.out_of_diff.is_empty() {
        out.push_str(&format!(
            "**Вне изменённых строк ({})** — привязать к строкам нельзя, поэтому здесь:\n\n",
            result.out_of_diff.len()
        ));
        out.push_str(&finding_list(&result.out_of_diff));
        out.push('\n');
    }

    if !result.unknown_path.is_empty() {
        out.push_str(&format!(
            "**Про файлы вне области скилла ({})**:\n\n",
            result.unknown_path.len()
        ));
        out.push_str(&finding_list(&result.unknown_path));
        out.push('\n');
    }

    if result.truncated > 0 {
        out.push_str(&format!(
            "Ещё {} замечаний не показаны: сработал лимит на число комментариев.\n\n",
            result.truncated
        ));
    }

    if result.is_empty() {
        out.push_str("Замечаний нет.\n\n");
    }

    out.push_str(&ctx.footer());
    out
}

/// Список находок в виде markdown-списка.
fn finding_list(findings: &[Finding]) -> String {
    let mut out = String::new();
    for finding in findings {
        let place = if finding.line == 0 {
            finding.path.clone()
        } else {
            format!("{}:{}", finding.path, finding.line)
        };
        out.push_str(&format!(
            "- `{place}` _{}_ — {}\n",
            finding.severity.as_str(),
            finding.body.trim()
        ));
    }
    out
}

/// Заголовок pull request с изменениями.
pub fn pr_title(
    skill: &str,
    source_pr: u64,
    args: &std::collections::BTreeMap<String, String>,
) -> String {
    let suffix = if args.is_empty() {
        String::new()
    } else {
        let joined = args
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" ({joined})")
    };
    format!("momulus: {skill}{suffix} для #{source_pr}")
}

/// Описание pull request с изменениями.
pub fn pr_body(
    ctx: &JobContext<'_>,
    source_pr: u64,
    summary: Option<&str>,
    files: &[String],
) -> String {
    let mut out = format!(
        "Сгенерировано командой `{}` в #{source_pr}.\n\n",
        ctx.command
    );

    match summary.map(str::trim).filter(|s| !s.is_empty()) {
        Some(summary) => {
            out.push_str(summary);
            out.push_str("\n\n");
        }
        None => out.push_str("Резюме от скилла нет.\n\n"),
    }

    out.push_str(&format!("**Изменённые файлы ({})**:\n\n", files.len()));
    for file in files {
        out.push_str(&format!("- `{file}`\n"));
    }
    out.push('\n');
    out.push_str(&ctx.footer());
    out
}

/// Ответ на команду, для которой нечего делать.
pub fn nothing_to_do(ctx: &JobContext<'_>, filters: &[String]) -> String {
    let filters = if filters.is_empty() {
        "весь репозиторий".to_string()
    } else {
        filters
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "Нечего делать: среди изменённых файлов PR нет ни одного под фильтр скилла ({filters}).\n{}",
        ctx.footer()
    )
}

/// Ответ patch-скилла, который ничего не изменил.
pub fn no_changes(ctx: &JobContext<'_>, summary: Option<&str>) -> String {
    let mut out = String::from("Скилл отработал, но изменений в файлах нет.\n\n");
    if let Some(summary) = summary.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(summary);
        out.push_str("\n\n");
    }
    out.push_str(&ctx.footer());
    out
}

/// Сообщение об ошибке: без стектрейсов и без секретов.
pub fn error_comment(ctx: &JobContext<'_>, reason: &str) -> String {
    format!(
        "Команда `{}` не выполнена: {}\n{}",
        ctx.command,
        reason.trim(),
        ctx.footer()
    )
}

/// Ответ на команду, которую не удалось разобрать.
pub fn help_comment(reason: &str, registry_help: &str) -> String {
    format!(
        "Не понял команду: {}\n\n{}",
        reason.trim(),
        registry_help.trim_end()
    )
}

/// Ответ на слишком большой вход.
pub fn too_large(ctx: &JobContext<'_>, reason: &str) -> String {
    format!(
        "Команда `{}` не выполнена: {}\nСузь изменения в PR или фильтр файлов в скилле.\n{}",
        ctx.command,
        reason.trim(),
        ctx.footer()
    )
}

/// Ответ на patch-команду в PR из форка.
pub fn fork_unsupported(ctx: &JobContext<'_>, head_repo: &str) -> String {
    format!(
        "Команда `{}` не выполнена: PR открыт из форка `{head_repo}`, \
         а у бота нет прав на запись в него. Скиллы в режиме patch работают \
         только для ветвей основного репозитория.\n{}",
        ctx.command,
        ctx.footer()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use momulus_core::{Severity, Uuid};
    use std::collections::BTreeMap;

    /// Фиксированный id, чтобы снапшоты не плыли.
    fn ctx<'a>(command: &'a str, skill: &'a str) -> JobContext<'a> {
        JobContext {
            job_id: JobId(Uuid::from_u128(0x1a2b3c4d_5e6f_7081_9202_a3b4c5d6e7f8)),
            skill,
            command,
        }
    }

    fn finding(path: &str, line: u32, severity: Severity, body: &str) -> Finding {
        Finding {
            path: path.to_string(),
            line,
            severity,
            body: body.to_string(),
            suggestion: None,
        }
    }

    #[test]
    fn review_summary_snapshot() {
        let result = ReviewResult {
            summary: "В статье много пунктуационных ошибок и раздельного написания\nтерминов с аббревиатурами.".into(),
            inline: vec![
                finding("posts/dns.md", 18, Severity::Typo, "Тавтология: «красивое красивое»"),
                finding("posts/dns.md", 178, Severity::Typo, "«требуемои» → «требуемое»"),
            ],
            out_of_diff: vec![finding(
                "posts/dns.md",
                420,
                Severity::Terminology,
                "«MacOS» → «macOS»",
            )],
            unknown_path: vec![finding(
                "astro.config.ts",
                10,
                Severity::Other,
                "Файл вне области скилла",
            )],
            truncated: 5,
        };
        insta::assert_snapshot!(
            "review_summary",
            review_summary(&result, &ctx("/llm proofread", "proofread"), 2)
        );
    }

    #[test]
    fn empty_review_summary_snapshot() {
        let result = ReviewResult {
            summary: "Всё чисто, замечаний нет.".into(),
            ..Default::default()
        };
        insta::assert_snapshot!(
            "review_summary_empty",
            review_summary(&result, &ctx("/llm proofread", "proofread"), 1)
        );
    }

    #[test]
    fn pr_body_snapshot() {
        let files = vec![
            "content/posts/dns/index.en.md".to_string(),
            "content/posts/act/index.en.md".to_string(),
        ];
        insta::assert_snapshot!(
            "pr_body",
            pr_body(
                &ctx("/llm translate en", "translate"),
                42,
                Some("Перевёл две статьи, термины и код-блоки оставил как есть."),
                &files
            )
        );
    }

    #[test]
    fn pr_title_includes_args() {
        let args = BTreeMap::from([("lang".to_string(), "en".to_string())]);
        assert_eq!(
            pr_title("translate", 42, &args),
            "momulus: translate (lang=en) для #42"
        );
        assert_eq!(
            pr_title("translate", 42, &BTreeMap::new()),
            "momulus: translate для #42"
        );
    }

    #[test]
    fn service_messages_snapshot() {
        let ctx = ctx("/llm proofread", "proofread");
        let mut out = String::new();
        out.push_str("=== нечего делать ===\n");
        out.push_str(&nothing_to_do(
            &ctx,
            &["**/*.md".to_string(), "**/*.mdx".to_string()],
        ));
        out.push_str("\n=== нет изменений ===\n");
        out.push_str(&no_changes(&ctx, Some("Всё уже переведено.")));
        out.push_str("\n=== ошибка ===\n");
        out.push_str(&error_comment(
            &ctx,
            "модель вернула невалидный JSON дважды",
        ));
        out.push_str("\n=== слишком много ===\n");
        out.push_str(&too_large(&ctx, "diff PR — 812 КБ при лимите 400 КБ"));
        out.push_str("\n=== форк ===\n");
        out.push_str(&fork_unsupported(&ctx, "contributor/blojik"));
        insta::assert_snapshot!("service_messages", out);
    }

    #[test]
    fn help_comment_snapshot() {
        insta::assert_snapshot!(
            "help_comment",
            help_comment(
                "неизвестный скилл \"proofraed\"",
                "Доступные команды:\n\n- `/llm proofread` (review) — вычитка\n- `/llm translate <lang>` (patch) — перевод\n\nФормат: `/llm <skill> [аргументы]`.\n"
            )
        );
    }

    #[test]
    fn error_comment_has_no_stack_traces() {
        let text = error_comment(&ctx("/llm review", "review"), "git завершился с кодом 128");
        assert!(!text.contains("panicked"), "{text}");
        assert!(text.contains("job `1a2b3c4d`"), "{text}");
    }
}

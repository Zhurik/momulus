//! Проверка того, что вернула модель, и приведение находок к публикуемому виду.

use std::collections::BTreeSet;

use momulus_core::{Error, Finding, Result, ReviewOutput};
use momulus_workspace::DiffIndex;

use crate::prompt::WORK_DIR;

/// Разобранные и разложенные по полочкам находки.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewResult {
    /// Резюме, как его написала модель.
    pub summary: String,
    /// Находки, которые можно повесить на строки diff'а.
    pub inline: Vec<Finding>,
    /// Находки на строках вне diff — уходят в текст резюме.
    pub out_of_diff: Vec<Finding>,
    /// Находки про файлы, которых скилл не касался.
    pub unknown_path: Vec<Finding>,
    /// Сколько находок отброшено из-за max_comments.
    pub truncated: usize,
}

impl ReviewResult {
    /// Всего находок после разбора (без отброшенных лимитом).
    pub fn kept(&self) -> usize {
        self.inline.len() + self.out_of_diff.len() + self.unknown_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kept() == 0 && self.truncated == 0
    }
}

/// Строго разбирает `findings.json`.
///
/// Единственная вольность — снимаем markdown-обёртку ```json, если модель
/// всё-таки её добавила: это дешевле, чем тратить повторный вызов.
pub fn parse_review(raw: &str) -> Result<ReviewOutput> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(Error::InvalidOutput("файл findings.json пустой".into()));
    }

    let first = serde_json::from_str::<ReviewOutput>(text);
    let err = match first {
        Ok(output) => return Ok(output),
        Err(err) => err,
    };

    if let Some(inner) = strip_code_fence(text)
        && let Ok(output) = serde_json::from_str::<ReviewOutput>(inner)
    {
        return Ok(output);
    }

    Err(Error::InvalidOutput(err.to_string()))
}

/// Возвращает содержимое первого ```-блока.
fn strip_code_fence(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after = &text[start + 3..];
    // Пропускаем возможный ярлык языка в первой строке.
    let after = match after.find('\n') {
        Some(nl) if after[..nl].trim().chars().all(char::is_alphanumeric) => &after[nl + 1..],
        _ => after,
    };
    let end = after.find("```")?;
    Some(after[..end].trim())
}

/// Раскладывает находки: что публикуем построчно, что уходит в резюме.
///
/// `diff` — diff PR; для локального прогона его нет, тогда строки не проверяем.
pub fn prepare_review(
    output: ReviewOutput,
    files: &[String],
    diff: Option<&DiffIndex>,
    max_comments: usize,
) -> ReviewResult {
    let allowed: BTreeSet<&str> = files.iter().map(String::as_str).collect();

    let mut findings: Vec<Finding> = output
        .findings
        .into_iter()
        .map(|mut finding| {
            finding.path = normalize_path(&finding.path);
            finding
        })
        .collect();
    // Детерминированный порядок: так же выглядит ревью в PR и снапшоты в тестах.
    findings.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));

    let mut result = ReviewResult {
        summary: output.summary,
        ..Default::default()
    };

    for finding in findings {
        if !allowed.is_empty() && !allowed.contains(finding.path.as_str()) {
            result.unknown_path.push(finding);
            continue;
        }
        if finding.line == 0 {
            result.out_of_diff.push(finding);
            continue;
        }
        match diff {
            Some(index) if !index.is_commentable(&finding.path, finding.line) => {
                result.out_of_diff.push(finding);
            }
            _ => result.inline.push(finding),
        }
    }

    if result.inline.len() > max_comments {
        result.truncated = result.inline.len() - max_comments;
        result.inline.truncate(max_comments);
    }
    result
}

/// Приводит путь от модели к пути относительно корня репозитория.
fn normalize_path(path: &str) -> String {
    let mut path = path.trim();
    for prefix in [
        concat!("/work", "/"),
        "work/",
        "./",
        // Иногда модель пишет путь с ведущим слешем от корня репозитория.
        "/",
    ] {
        if let Some(stripped) = path.strip_prefix(prefix) {
            path = stripped;
        }
    }
    debug_assert!(WORK_DIR == "/work");
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use momulus_core::Severity;

    fn finding(path: &str, line: u32, body: &str) -> Finding {
        Finding {
            path: path.to_string(),
            line,
            severity: Severity::Typo,
            body: body.to_string(),
            suggestion: None,
        }
    }

    const DIFF: &str = "\
diff --git a/posts/a.mdx b/posts/a.mdx
--- a/posts/a.mdx
+++ b/posts/a.mdx
@@ -1,3 +1,4 @@
 первая
-вторая
+ВТОРАЯ
+третья
 четвёртая
";

    #[test]
    fn parses_valid_json() {
        let out = parse_review(
            r#"{"summary":"итог","findings":[{"path":"a.mdx","line":2,"severity":"typo","body":"опечатка"}]}"#,
        )
        .unwrap();
        assert_eq!(out.summary, "итог");
        assert_eq!(out.findings.len(), 1);
    }

    #[test]
    fn rejects_unknown_severity() {
        let err = parse_review(
            r#"{"summary":"s","findings":[{"path":"a.mdx","line":1,"severity":"нытьё","body":"b"}]}"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidOutput(_)), "{err:?}");
        assert!(err.to_string().contains("unknown variant"), "{err}");
        // В сообщении для модели перечислены допустимые значения.
        assert!(err.to_string().contains("punctuation"), "{err}");
    }

    #[test]
    fn rejects_extra_fields() {
        let err = parse_review(r#"{"summary":"s","findings":[],"extra":1}"#).unwrap_err();
        assert!(err.to_string().contains("extra"), "{err}");
    }

    #[test]
    fn rejects_missing_summary() {
        let err = parse_review(r#"{"findings":[]}"#).unwrap_err();
        assert!(err.to_string().contains("summary"), "{err}");
    }

    #[test]
    fn rejects_empty_file() {
        let err = parse_review("   \n").unwrap_err();
        assert!(err.to_string().contains("пустой"), "{err}");
    }

    #[test]
    fn rejects_plain_text() {
        let err = parse_review("я не смог, извини").unwrap_err();
        assert!(matches!(err, Error::InvalidOutput(_)));
    }

    #[test]
    fn tolerates_markdown_code_fence() {
        let out = parse_review("```json\n{\"summary\":\"s\",\"findings\":[]}\n```").unwrap();
        assert_eq!(out.summary, "s");
        let out = parse_review("вот результат:\n```\n{\"summary\":\"s2\",\"findings\":[]}\n```\n")
            .unwrap();
        assert_eq!(out.summary, "s2");
    }

    #[test]
    fn fence_with_broken_json_still_fails() {
        let err = parse_review("```json\n{\"summary\":}\n```").unwrap_err();
        assert!(matches!(err, Error::InvalidOutput(_)));
    }

    #[test]
    fn normalizes_paths_from_the_container() {
        assert_eq!(normalize_path("/work/posts/a.mdx"), "posts/a.mdx");
        assert_eq!(normalize_path("./posts/a.mdx"), "posts/a.mdx");
        assert_eq!(normalize_path("/posts/a.mdx"), "posts/a.mdx");
        assert_eq!(normalize_path(" posts/a.mdx "), "posts/a.mdx");
        assert_eq!(normalize_path("posts/a.mdx"), "posts/a.mdx");
    }

    #[test]
    fn keeps_findings_inside_the_diff() {
        let diff = DiffIndex::parse(DIFF).unwrap();
        let output = ReviewOutput {
            summary: "итог".into(),
            findings: vec![
                finding("/work/posts/a.mdx", 2, "в diff"),
                finding("posts/a.mdx", 3, "тоже в diff"),
            ],
        };
        let result = prepare_review(output, &["posts/a.mdx".into()], Some(&diff), 30);
        assert_eq!(result.inline.len(), 2);
        assert!(result.out_of_diff.is_empty());
        assert!(result.unknown_path.is_empty());
    }

    #[test]
    fn moves_findings_outside_the_diff_to_summary() {
        let diff = DiffIndex::parse(DIFF).unwrap();
        let output = ReviewOutput {
            summary: "итог".into(),
            findings: vec![
                finding("posts/a.mdx", 2, "в diff"),
                finding("posts/a.mdx", 99, "вне hunk"),
                finding("posts/a.mdx", 0, "без строки"),
            ],
        };
        let result = prepare_review(output, &["posts/a.mdx".into()], Some(&diff), 30);
        assert_eq!(result.inline.len(), 1);
        assert_eq!(result.out_of_diff.len(), 2);
        assert_eq!(result.kept(), 3, "ничего не потеряли");
    }

    #[test]
    fn moves_findings_about_other_files_to_summary() {
        let diff = DiffIndex::parse(DIFF).unwrap();
        let output = ReviewOutput {
            summary: "итог".into(),
            findings: vec![
                finding("posts/a.mdx", 2, "наш файл"),
                finding("src/main.rs", 10, "чужой файл"),
            ],
        };
        let result = prepare_review(output, &["posts/a.mdx".into()], Some(&diff), 30);
        assert_eq!(result.inline.len(), 1);
        assert_eq!(result.unknown_path.len(), 1);
        assert_eq!(result.unknown_path[0].path, "src/main.rs");
    }

    #[test]
    fn truncates_to_max_comments() {
        let diff = DiffIndex::parse(DIFF).unwrap();
        let output = ReviewOutput {
            summary: "итог".into(),
            findings: (2..=4)
                .map(|line| finding("posts/a.mdx", line, "замечание"))
                .collect(),
        };
        let result = prepare_review(output, &["posts/a.mdx".into()], Some(&diff), 2);
        assert_eq!(result.inline.len(), 2);
        assert_eq!(result.truncated, 1);
    }

    #[test]
    fn without_diff_all_lines_are_commentable() {
        let output = ReviewOutput {
            summary: "итог".into(),
            findings: vec![finding("posts/a.mdx", 1234, "локальный прогон")],
        };
        let result = prepare_review(output, &["posts/a.mdx".into()], None, 30);
        assert_eq!(result.inline.len(), 1);
    }

    #[test]
    fn empty_file_list_allows_any_path() {
        let output = ReviewOutput {
            summary: "итог".into(),
            findings: vec![finding("что/угодно.md", 1, "b")],
        };
        let result = prepare_review(output, &[], None, 30);
        assert_eq!(result.inline.len(), 1);
    }

    #[test]
    fn findings_are_sorted_by_path_and_line() {
        let output = ReviewOutput {
            summary: "итог".into(),
            findings: vec![
                finding("b.mdx", 5, "раз"),
                finding("a.mdx", 10, "два"),
                finding("a.mdx", 2, "три"),
            ],
        };
        let result = prepare_review(output, &[], None, 30);
        let order: Vec<(&str, u32)> = result
            .inline
            .iter()
            .map(|f| (f.path.as_str(), f.line))
            .collect();
        assert_eq!(order, vec![("a.mdx", 2), ("a.mdx", 10), ("b.mdx", 5)]);
    }

    #[test]
    fn empty_review_is_detected() {
        let output = ReviewOutput {
            summary: "всё чисто".into(),
            findings: Vec::new(),
        };
        let result = prepare_review(output, &[], None, 30);
        assert!(result.is_empty());
        assert_eq!(result.summary, "всё чисто");
    }
}

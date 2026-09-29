//! Validating what the model returned and shaping the findings for publication.

use std::collections::BTreeSet;

use momulus_core::{Error, Finding, Result, ReviewOutput};
use momulus_workspace::DiffIndex;

use crate::prompt::WORK_DIR;

/// Findings, parsed and sorted into their proper buckets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewResult {
    /// The summary exactly as the model wrote it.
    pub summary: String,
    /// Findings that can be anchored to lines of the diff.
    pub inline: Vec<Finding>,
    /// Findings on lines outside the diff — they go into the summary text.
    pub out_of_diff: Vec<Finding>,
    /// Findings about files the skill never touched.
    pub unknown_path: Vec<Finding>,
    /// How many findings were dropped because of max_comments.
    pub truncated: usize,
}

impl ReviewResult {
    /// Total findings after parsing (excluding the ones dropped by the limit).
    pub fn kept(&self) -> usize {
        self.inline.len() + self.out_of_diff.len() + self.unknown_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kept() == 0 && self.truncated == 0
    }
}

/// Strictly parses `findings.json`.
///
/// The one liberty we take is stripping a ```json markdown fence if the model
/// added one anyway: cheaper than spending another model call.
pub fn parse_review(raw: &str) -> Result<ReviewOutput> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(Error::InvalidOutput(
            "the findings.json file is empty".into(),
        ));
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

/// Returns the contents of the first ``` block.
fn strip_code_fence(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after = &text[start + 3..];
    // Skip a possible language tag on the first line.
    let after = match after.find('\n') {
        Some(nl) if after[..nl].trim().chars().all(char::is_alphanumeric) => &after[nl + 1..],
        _ => after,
    };
    let end = after.find("```")?;
    Some(after[..end].trim())
}

/// Sorts findings: what gets an inline comment and what goes into the summary.
///
/// `diff` is the PR diff; a local run has none, and then lines are not checked.
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
    // Deterministic order: the review in the PR and the test snapshots match.
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

/// Normalises a path from the model into a repository-relative one.
fn normalize_path(path: &str) -> String {
    let mut path = path.trim();
    for prefix in [
        concat!("/work", "/"),
        "work/",
        "./",
        // Sometimes the model writes a leading slash relative to the repo root.
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
 first
-second
+SECOND
+third
 fourth
";

    #[test]
    fn parses_valid_json() {
        let out = parse_review(
            r#"{"summary":"wrap-up","findings":[{"path":"a.mdx","line":2,"severity":"typo","body":"typo"}]}"#,
        )
        .unwrap();
        assert_eq!(out.summary, "wrap-up");
        assert_eq!(out.findings.len(), 1);
    }

    #[test]
    fn rejects_unknown_severity() {
        let err = parse_review(
            r#"{"summary":"s","findings":[{"path":"a.mdx","line":1,"severity":"whining","body":"b"}]}"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidOutput(_)), "{err:?}");
        assert!(err.to_string().contains("unknown variant"), "{err}");
        // The message shown to the model lists the accepted values.
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
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn rejects_plain_text() {
        let err = parse_review("sorry, I could not do it").unwrap_err();
        assert!(matches!(err, Error::InvalidOutput(_)));
    }

    #[test]
    fn tolerates_markdown_code_fence() {
        let out = parse_review("```json\n{\"summary\":\"s\",\"findings\":[]}\n```").unwrap();
        assert_eq!(out.summary, "s");
        let out =
            parse_review("here is the result:\n```\n{\"summary\":\"s2\",\"findings\":[]}\n```\n")
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
            summary: "wrap-up".into(),
            findings: vec![
                finding("/work/posts/a.mdx", 2, "inside the diff"),
                finding("posts/a.mdx", 3, "also inside the diff"),
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
            summary: "wrap-up".into(),
            findings: vec![
                finding("posts/a.mdx", 2, "inside the diff"),
                finding("posts/a.mdx", 99, "outside the hunk"),
                finding("posts/a.mdx", 0, "no line at all"),
            ],
        };
        let result = prepare_review(output, &["posts/a.mdx".into()], Some(&diff), 30);
        assert_eq!(result.inline.len(), 1);
        assert_eq!(result.out_of_diff.len(), 2);
        assert_eq!(result.kept(), 3, "nothing was lost");
    }

    #[test]
    fn moves_findings_about_other_files_to_summary() {
        let diff = DiffIndex::parse(DIFF).unwrap();
        let output = ReviewOutput {
            summary: "wrap-up".into(),
            findings: vec![
                finding("posts/a.mdx", 2, "our file"),
                finding("src/main.rs", 10, "somebody else's file"),
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
            summary: "wrap-up".into(),
            findings: (2..=4)
                .map(|line| finding("posts/a.mdx", line, "a finding"))
                .collect(),
        };
        let result = prepare_review(output, &["posts/a.mdx".into()], Some(&diff), 2);
        assert_eq!(result.inline.len(), 2);
        assert_eq!(result.truncated, 1);
    }

    #[test]
    fn without_diff_all_lines_are_commentable() {
        let output = ReviewOutput {
            summary: "wrap-up".into(),
            findings: vec![finding("posts/a.mdx", 1234, "local run")],
        };
        let result = prepare_review(output, &["posts/a.mdx".into()], None, 30);
        assert_eq!(result.inline.len(), 1);
    }

    #[test]
    fn empty_file_list_allows_any_path() {
        let output = ReviewOutput {
            summary: "wrap-up".into(),
            findings: vec![finding("any/path.md", 1, "b")],
        };
        let result = prepare_review(output, &[], None, 30);
        assert_eq!(result.inline.len(), 1);
    }

    #[test]
    fn findings_are_sorted_by_path_and_line() {
        let output = ReviewOutput {
            summary: "wrap-up".into(),
            findings: vec![
                finding("b.mdx", 5, "one"),
                finding("a.mdx", 10, "two"),
                finding("a.mdx", 2, "three"),
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
            summary: "all clean".into(),
            findings: Vec::new(),
        };
        let result = prepare_review(output, &[], None, 30);
        assert!(result.is_empty());
        assert_eq!(result.summary, "all clean");
    }
}

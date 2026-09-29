//! Unified diff parsing and mapping of line numbers onto hunks.
//!
//! Serves two purposes: selecting the changed files a skill cares about, and
//! deciding whether a line can carry an inline comment (the platform only
//! accepts lines that appear in the diff).

use std::collections::BTreeMap;

use momulus_core::{Error, Result};

/// What happened to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// Kind of a line inside a hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// A hunk line with its numbers in the old and the new version of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: String,
}

/// A single hunk: `@@ -a,b +c,d @@`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// Range of new-version lines covered by the hunk.
    pub fn new_range(&self) -> std::ops::RangeInclusive<u32> {
        let end = if self.new_lines == 0 {
            self.new_start
        } else {
            self.new_start + self.new_lines - 1
        };
        self.new_start..=end
    }

    /// Whether the hunk contains a line with this number in the new version.
    pub fn covers_new_line(&self, line: u32) -> bool {
        self.lines
            .iter()
            .any(|l| l.new_line == Some(line) && l.kind != LineKind::Removed)
    }
}

/// Diff of a single file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Path in the new version (for a deleted file, the path before deletion).
    pub path: String,
    /// Previous path when the file was renamed.
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// Lines added or changed in the new version.
    pub fn added_lines(&self) -> Vec<u32> {
        self.hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .filter(|l| l.kind == LineKind::Added)
            .filter_map(|l| l.new_line)
            .collect()
    }

    /// Whether a line can be commented on: it must appear on the right side of the diff.
    pub fn covers_new_line(&self, line: u32) -> bool {
        self.hunks.iter().any(|h| h.covers_new_line(line))
    }
}

/// Parses `git diff` output into a list of files.
pub fn parse_unified_diff(text: &str) -> Result<Vec<FileDiff>> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current: Option<FileDiff> = None;
    let mut hunk: Option<Hunk> = None;
    let mut old_line = 0u32;
    let mut new_line = 0u32;

    /// Closes the current hunk and attaches it to the file.
    fn flush_hunk(current: &mut Option<FileDiff>, hunk: &mut Option<Hunk>) {
        if let (Some(file), Some(h)) = (current.as_mut(), hunk.take()) {
            file.hunks.push(h);
        }
    }

    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            flush_hunk(&mut current, &mut hunk);
            if let Some(file) = current.take() {
                files.push(file);
            }
            let path = parse_diff_header(rest)?;
            current = Some(FileDiff {
                path,
                old_path: None,
                status: FileStatus::Modified,
                binary: false,
                hunks: Vec::new(),
            });
            continue;
        }

        let Some(file) = current.as_mut() else {
            // Anything before the first header (a commit message, say) is ignored.
            continue;
        };

        if raw.starts_with("new file mode") {
            file.status = FileStatus::Added;
        } else if raw.starts_with("deleted file mode") {
            file.status = FileStatus::Deleted;
        } else if let Some(old) = raw.strip_prefix("rename from ") {
            file.status = FileStatus::Renamed;
            file.old_path = Some(unquote_path(old));
        } else if let Some(new) = raw.strip_prefix("rename to ") {
            file.status = FileStatus::Renamed;
            file.path = unquote_path(new);
        } else if raw.starts_with("Binary files ") || raw.starts_with("GIT binary patch") {
            file.binary = true;
        } else if let Some(tail) = raw.strip_prefix("+++ ") {
            // The path from the header is more reliable when the name has spaces.
            if let Some(path) = strip_prefix_marker(tail)
                && file.status != FileStatus::Deleted
            {
                file.path = path;
            }
        } else if let Some(tail) = raw.strip_prefix("--- ") {
            if let Some(path) = strip_prefix_marker(tail)
                && file.status == FileStatus::Deleted
            {
                file.path = path;
            }
        } else if raw.starts_with("@@") {
            flush_hunk(&mut current, &mut hunk);
            let header = parse_hunk_header(raw)?;
            old_line = header.old_start;
            new_line = header.new_start;
            hunk = Some(header);
        } else if let Some(h) = hunk.as_mut() {
            let (kind, text) = match raw.chars().next() {
                Some('+') => (LineKind::Added, &raw[1..]),
                Some('-') => (LineKind::Removed, &raw[1..]),
                Some(' ') => (LineKind::Context, &raw[1..]),
                Some('\\') => continue, // "\ No newline at end of file"
                None => (LineKind::Context, ""),
                _ => continue, // Trailing output such as "-- " from git format-patch.
            };
            let (old, new) = match kind {
                LineKind::Added => {
                    let n = new_line;
                    new_line += 1;
                    (None, Some(n))
                }
                LineKind::Removed => {
                    let o = old_line;
                    old_line += 1;
                    (Some(o), None)
                }
                LineKind::Context => {
                    let (o, n) = (old_line, new_line);
                    old_line += 1;
                    new_line += 1;
                    (Some(o), Some(n))
                }
            };
            h.lines.push(DiffLine {
                kind,
                old_line: old,
                new_line: new,
                text: text.to_string(),
            });
        }
    }

    flush_hunk(&mut current, &mut hunk);
    if let Some(file) = current.take() {
        files.push(file);
    }
    Ok(files)
}

/// Extracts the path from an `a/path b/path` line.
fn parse_diff_header(rest: &str) -> Result<String> {
    // Names may contain spaces, so we lean on the a/ and b/ prefixes.
    if let Some(b_at) = rest.rfind(" b/") {
        return Ok(unquote_path(&rest[b_at + 3..]));
    }
    if let Some(path) = rest.split_whitespace().next_back() {
        return Ok(unquote_path(path.trim_start_matches("b/")));
    }
    Err(Error::Git(format!("unparseable diff header: {rest:?}")))
}

/// Strips the `a/` or `b/` prefix from `---`/`+++` lines; `/dev/null` yields `None`.
fn strip_prefix_marker(path: &str) -> Option<String> {
    let path = path.split('\t').next().unwrap_or(path).trim_end();
    if path == "/dev/null" {
        return None;
    }
    let path = unquote_path(path);
    for prefix in ["a/", "b/"] {
        if let Some(stripped) = path.strip_prefix(prefix) {
            return Some(stripped.to_string());
        }
    }
    Some(path)
}

/// Removes the quotes git puts around non-ASCII paths.
fn unquote_path(path: &str) -> String {
    let path = path.trim();
    if path.len() >= 2 && path.starts_with('"') && path.ends_with('"') {
        let inner = &path[1..path.len() - 1];
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    path.to_string()
}

/// Parses `@@ -a,b +c,d @@`.
fn parse_hunk_header(line: &str) -> Result<Hunk> {
    let bad = || Error::Git(format!("unparseable hunk header: {line:?}"));
    let body = line.trim_start_matches('@').trim();
    let body = body.split("@@").next().ok_or_else(bad)?.trim();
    let mut parts = body.split_whitespace();
    let old = parts.next().ok_or_else(bad)?;
    let new = parts.next().ok_or_else(bad)?;

    let (old_start, old_lines) =
        parse_range(old.strip_prefix('-').ok_or_else(bad)?).ok_or_else(bad)?;
    let (new_start, new_lines) =
        parse_range(new.strip_prefix('+').ok_or_else(bad)?).ok_or_else(bad)?;

    Ok(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines: Vec::new(),
    })
}

fn parse_range(text: &str) -> Option<(u32, u32)> {
    match text.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((text.parse().ok()?, 1)),
    }
}

/// Index over a PR diff: fast lookup by file path.
#[derive(Debug, Clone, Default)]
pub struct DiffIndex {
    files: BTreeMap<String, FileDiff>,
}

impl DiffIndex {
    pub fn parse(text: &str) -> Result<DiffIndex> {
        Ok(DiffIndex::from_files(parse_unified_diff(text)?))
    }

    pub fn from_files(files: Vec<FileDiff>) -> DiffIndex {
        DiffIndex {
            files: files
                .into_iter()
                .map(|file| (file.path.clone(), file))
                .collect(),
        }
    }

    /// Every file in the diff.
    pub fn files(&self) -> impl Iterator<Item = &FileDiff> {
        self.files.values()
    }

    /// Paths of changed files, excluding deleted and binary ones — reading those is pointless.
    pub fn reviewable_files(&self) -> Vec<&str> {
        self.files
            .values()
            .filter(|f| f.status != FileStatus::Deleted && !f.binary)
            .map(|f| f.path.as_str())
            .collect()
    }

    pub fn get(&self, path: &str) -> Option<&FileDiff> {
        self.files.get(path)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether a line can carry an inline comment: the platform only accepts
    /// lines that appear in the diff.
    pub fn is_commentable(&self, path: &str, line: u32) -> bool {
        self.files
            .get(path)
            .is_some_and(|file| file.covers_new_line(line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &str = "\
diff --git a/posts/hello.mdx b/posts/hello.mdx
index 1111111..2222222 100644
--- a/posts/hello.mdx
+++ b/posts/hello.mdx
@@ -1,4 +1,5 @@
 # Hello
 
-Old line
+New line
+One more
 Tail
";

    #[test]
    fn parses_single_file() {
        let files = parse_unified_diff(SIMPLE).unwrap();
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.path, "posts/hello.mdx");
        assert_eq!(file.status, FileStatus::Modified);
        assert_eq!(file.hunks.len(), 1);
    }

    #[test]
    fn maps_line_numbers() {
        let files = parse_unified_diff(SIMPLE).unwrap();
        let hunk = &files[0].hunks[0];
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.new_start, 1);
        let added: Vec<(u32, &str)> = hunk
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Added)
            .map(|l| (l.new_line.unwrap(), l.text.as_str()))
            .collect();
        assert_eq!(added, vec![(3, "New line"), (4, "One more")]);

        let removed: Vec<(u32, &str)> = hunk
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Removed)
            .map(|l| (l.old_line.unwrap(), l.text.as_str()))
            .collect();
        assert_eq!(removed, vec![(3, "Old line")]);
    }

    #[test]
    fn context_lines_carry_both_numbers() {
        let files = parse_unified_diff(SIMPLE).unwrap();
        let last = files[0].hunks[0].lines.last().unwrap();
        assert_eq!(last.kind, LineKind::Context);
        assert_eq!(last.old_line, Some(4));
        assert_eq!(last.new_line, Some(5));
    }

    #[test]
    fn detects_added_and_deleted_files() {
        let text = "\
diff --git a/new.md b/new.md
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/new.md
@@ -0,0 +1,2 @@
+one
+two
diff --git a/gone.md b/gone.md
deleted file mode 100644
index 1111111..0000000
--- a/gone.md
+++ /dev/null
@@ -1,1 +0,0 @@
-gone
";
        let files = parse_unified_diff(text).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "new.md");
        assert_eq!(files[0].status, FileStatus::Added);
        assert_eq!(files[0].added_lines(), vec![1, 2]);
        assert_eq!(files[1].path, "gone.md");
        assert_eq!(files[1].status, FileStatus::Deleted);
    }

    #[test]
    fn detects_renames() {
        let text = "\
diff --git a/old.md b/new.md
similarity index 90%
rename from old.md
rename to new.md
--- a/old.md
+++ b/new.md
@@ -1 +1 @@
-one
+two
";
        let files = parse_unified_diff(text).unwrap();
        assert_eq!(files[0].status, FileStatus::Renamed);
        assert_eq!(files[0].path, "new.md");
        assert_eq!(files[0].old_path.as_deref(), Some("old.md"));
    }

    #[test]
    fn detects_binary_files() {
        let text = "\
diff --git a/img.png b/img.png
index 1111111..2222222 100644
Binary files a/img.png and b/img.png differ
";
        let files = parse_unified_diff(text).unwrap();
        assert!(files[0].binary);
        let index = DiffIndex::from_files(files);
        assert!(index.reviewable_files().is_empty());
    }

    #[test]
    fn handles_multiple_hunks() {
        let text = "\
diff --git a/a.md b/a.md
--- a/a.md
+++ b/a.md
@@ -1,2 +1,2 @@
 one
-two
+TWO
@@ -10,3 +10,4 @@
 ten
 eleven
+inserted
 twelve
";
        let files = parse_unified_diff(text).unwrap();
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[0].added_lines(), vec![2, 12]);
        assert_eq!(files[0].hunks[1].new_range(), 10..=13);
    }

    #[test]
    fn handles_paths_with_spaces_and_unicode() {
        let text = "\
diff --git a/posts/über kätzchen.mdx b/posts/über kätzchen.mdx
--- a/posts/über kätzchen.mdx	
+++ b/posts/über kätzchen.mdx	
@@ -1 +1 @@
-one
+two
";
        let files = parse_unified_diff(text).unwrap();
        assert_eq!(files[0].path, "posts/über kätzchen.mdx");
    }

    #[test]
    fn ignores_no_newline_marker() {
        let text = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-one
\\ No newline at end of file
+two
\\ No newline at end of file
";
        let files = parse_unified_diff(text).unwrap();
        assert_eq!(files[0].added_lines(), vec![1]);
    }

    #[test]
    fn single_line_hunk_header_without_count() {
        let text = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -5 +5 @@ fn main()
-one
+two
";
        let files = parse_unified_diff(text).unwrap();
        let hunk = &files[0].hunks[0];
        assert_eq!((hunk.new_start, hunk.new_lines), (5, 1));
        assert_eq!(files[0].added_lines(), vec![5]);
    }

    #[test]
    fn rejects_broken_hunk_header() {
        let text = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ nonsense @@
";
        assert!(parse_unified_diff(text).is_err());
    }

    #[test]
    fn empty_diff_is_empty() {
        assert!(parse_unified_diff("").unwrap().is_empty());
        assert!(DiffIndex::parse("").unwrap().is_empty());
    }

    #[test]
    fn index_answers_commentable_lines() {
        let index = DiffIndex::parse(SIMPLE).unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(index.reviewable_files(), vec!["posts/hello.mdx"]);
        // Added lines.
        assert!(index.is_commentable("posts/hello.mdx", 3));
        assert!(index.is_commentable("posts/hello.mdx", 4));
        // Context lines inside the hunk work too.
        assert!(index.is_commentable("posts/hello.mdx", 1));
        assert!(index.is_commentable("posts/hello.mdx", 5));
        // Outside the hunk, or an unknown file — not allowed.
        assert!(!index.is_commentable("posts/hello.mdx", 99));
        assert!(!index.is_commentable("other.mdx", 1));
    }

    #[test]
    fn removed_line_number_is_not_commentable_on_the_right() {
        let text = "\
diff --git a/a.md b/a.md
--- a/a.md
+++ b/a.md
@@ -1,3 +1,2 @@
 one
-two
 three
";
        let index = DiffIndex::parse(text).unwrap();
        // Lines 1 and 2 exist in the new version, line 3 does not.
        assert!(index.is_commentable("a.md", 1));
        assert!(index.is_commentable("a.md", 2));
        assert!(!index.is_commentable("a.md", 3));
    }
}

//! Input size limits: protection against huge PRs and huge files.

use std::path::Path;

use momulus_core::config::Limits;
use momulus_core::{Error, Result};

/// A PR file and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSize {
    pub path: String,
    pub bytes: u64,
}

/// Checks the input before calling the model: refusing early is cheaper.
pub fn check_input(diff_bytes: u64, files: &[FileSize], limits: &Limits) -> Result<()> {
    if files.len() > limits.max_changed_files {
        return Err(Error::InputTooLarge(format!(
            "{} files match the skill, the limit is {}",
            files.len(),
            limits.max_changed_files
        )));
    }
    if diff_bytes > limits.max_diff_bytes {
        return Err(Error::InputTooLarge(format!(
            "the diff is {} against a {} limit",
            human_size(diff_bytes),
            human_size(limits.max_diff_bytes)
        )));
    }
    if let Some(big) = files.iter().find(|f| f.bytes > limits.max_file_bytes) {
        return Err(Error::InputTooLarge(format!(
            "the file `{}` is {} against a {} limit",
            big.path,
            human_size(big.bytes),
            human_size(limits.max_file_bytes)
        )));
    }
    Ok(())
}

/// Sizes of files in the working copy; missing files count as empty
/// (deleted in the PR, for instance).
pub fn measure_files(root: &Path, files: &[String]) -> Vec<FileSize> {
    files
        .iter()
        .map(|path| FileSize {
            path: path.clone(),
            bytes: std::fs::metadata(root.join(path))
                .map(|m| m.len())
                .unwrap_or(0),
        })
        .collect()
}

/// A human-readable size in KB/MB.
pub fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    match bytes {
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{} KB", b / KB),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            max_diff_bytes: 10_000,
            max_file_bytes: 5_000,
            max_changed_files: 3,
        }
    }

    fn file(path: &str, bytes: u64) -> FileSize {
        FileSize {
            path: path.into(),
            bytes,
        }
    }

    #[test]
    fn accepts_input_within_limits() {
        check_input(9_999, &[file("a.md", 4_999)], &limits()).unwrap();
    }

    #[test]
    fn rejects_too_many_files() {
        let files: Vec<FileSize> = (0..4).map(|i| file(&format!("f{i}.md"), 10)).collect();
        let err = check_input(100, &files, &limits()).unwrap_err();
        assert!(matches!(err, Error::InputTooLarge(_)), "{err:?}");
        assert!(err.to_string().contains("the limit is 3"), "{err}");
    }

    #[test]
    fn rejects_too_big_diff() {
        let err = check_input(20_000, &[], &limits()).unwrap_err();
        assert!(err.to_string().contains("19 KB"), "{err}");
        assert!(err.to_string().contains("9 KB"), "{err}");
    }

    #[test]
    fn rejects_too_big_file_and_names_it() {
        let err = check_input(100, &[file("posts/huge.md", 6_000)], &limits()).unwrap_err();
        assert!(err.to_string().contains("posts/huge.md"), "{err}");
    }

    #[test]
    fn input_too_large_is_permanent() {
        let err = check_input(20_000, &[], &limits()).unwrap_err();
        assert!(!err.is_transient(), "retrying is pointless");
    }

    #[test]
    fn measures_files_and_tolerates_missing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.md"), "12345").unwrap();
        let sizes = measure_files(tmp.path(), &["a.md".to_string(), "missing.md".to_string()]);
        assert_eq!(sizes[0], file("a.md", 5));
        assert_eq!(sizes[1], file("missing.md", 0));
    }

    #[test]
    fn human_size_reads_well() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }
}

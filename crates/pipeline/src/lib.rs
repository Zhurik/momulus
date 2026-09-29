//! Оркестрация одной джобы: рабочая копия → скилл → runner → валидация → публикация.

pub mod fake;
pub mod limits;
pub mod prompt;
pub mod render;
pub mod steps;
pub mod validate;

pub use fake::{FakeResponse, FakeRunner};
pub use limits::{FileSize, check_input, measure_files};
pub use prompt::{FINDINGS_FILE, OUT_DIR, Origin, PromptContext, SUMMARY_FILE, WORK_DIR};
pub use render::JobContext;
pub use steps::{PatchStep, ReviewStep, collect_patch, combined_log, run_patch, run_review};
pub use validate::{ReviewResult, parse_review, prepare_review};

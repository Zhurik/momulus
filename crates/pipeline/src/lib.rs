//! Оркестрация одной джобы: рабочая копия → скилл → runner → валидация → публикация.

pub mod fake;
pub mod prompt;

pub use fake::{FakeResponse, FakeRunner};
pub use prompt::{FINDINGS_FILE, OUT_DIR, Origin, PromptContext, SUMMARY_FILE, WORK_DIR};

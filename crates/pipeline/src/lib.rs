//! Orchestration of one job: working copy → skill → runner → validation → publication.

pub mod fake;
pub mod job;
pub mod limits;
pub mod prompt;
pub mod registry;
pub mod render;
pub mod stdout;
pub mod steps;
pub mod validate;

pub use fake::{FakeResponse, FakeRunner};
pub use job::{JobResult, Outcome, Pipeline, PipelineConfig};
pub use limits::{FileSize, check_input, measure_files};
pub use prompt::{FINDINGS_FILE, OUT_DIR, Origin, PromptContext, SUMMARY_FILE, WORK_DIR};
pub use registry::SharedRegistry;
pub use render::JobContext;
pub use stdout::StdoutPublisher;
pub use steps::{PatchStep, ReviewStep, collect_patch, combined_log, run_patch, run_review};
pub use validate::{ReviewResult, parse_review, prepare_review};

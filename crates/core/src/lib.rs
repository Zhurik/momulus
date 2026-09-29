//! Momulus core: types, traits and configuration.
//!
//! The crate knows nothing about GitHub or Docker — platform details live behind
//! the [`Trigger`], [`Publisher`] and [`Runner`] traits.

pub mod command;
pub mod config;
pub mod error;
pub mod memstore;
pub mod redact;
pub mod traits;
pub mod types;

pub use command::{Args, Command, CommandParseError};
pub use config::{Config, ProviderApi, Secrets};
pub use error::{Error, ErrorKind, Result};
pub use memstore::{LocalGitAccess, MemoryCursorStore, StaticSkillCatalog};
pub use redact::Redactor;
pub use traits::{CursorStore, GitAccess, Publisher, Runner, SkillCatalog, Trigger};
pub use types::{
    AckState, CommentKind, CommentRef, Finding, Job, JobId, JobRef, JobStatus, Mount, Patch, PrRef,
    ReviewOutput, RunResult, RunSpec, Severity,
};
pub use uuid::Uuid;

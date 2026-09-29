//! Ядро llm-bot: типы, трейты и конфигурация.
//!
//! Крейт не знает ни про GitHub, ни про Docker — платформенные детали живут
//! за трейтами [`Trigger`], [`Publisher`] и [`Runner`].

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
pub use memstore::{MemoryCursorStore, StaticSkillCatalog};
pub use redact::Redactor;
pub use traits::{CursorStore, Publisher, Runner, SkillCatalog, Trigger};
pub use types::{
    AckState, CommentKind, CommentRef, Finding, Job, JobId, JobRef, JobStatus, Mount, Patch, PrRef,
    ReviewOutput, RunResult, RunSpec, Severity,
};
pub use uuid::Uuid;

//! The skill registry and skill contracts.

pub mod contract;
pub mod doc;
pub mod error;
pub mod registry;

pub use contract::{DEFAULT_BRANCH_TEMPLATE, DEFAULT_MAX_COMMENTS, Mode, SkillContract};
pub use doc::SkillDoc;
pub use error::{SkillError, SkillResult};
pub use registry::{CONTRACT_FILE, DOC_FILE, LoadReport, Registry, Skill};

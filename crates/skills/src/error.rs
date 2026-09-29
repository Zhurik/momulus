//! Skill registry errors.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkillError {
    #[error("skill \"{skill}\": invalid skill.toml: {message}")]
    Contract { skill: String, message: String },

    #[error("skill \"{skill}\": invalid SKILL.md: {message}")]
    Doc { skill: String, message: String },

    #[error("skill \"{skill}\": {message}")]
    Args { skill: String, message: String },

    #[error("skills directory {}: {message}", path.display())]
    Registry { path: PathBuf, message: String },

    #[error("unknown skill \"{0}\"")]
    Unknown(String),

    #[error("{0}")]
    Internal(String),
}

pub type SkillResult<T> = Result<T, SkillError>;

impl From<SkillError> for momulus_core::Error {
    fn from(err: SkillError) -> Self {
        momulus_core::Error::Skill(err.to_string())
    }
}

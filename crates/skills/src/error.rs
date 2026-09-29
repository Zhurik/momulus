//! Ошибки реестра скиллов.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkillError {
    #[error("скилл \"{skill}\": невалидный skill.toml: {message}")]
    Contract { skill: String, message: String },

    #[error("скилл \"{skill}\": невалидный SKILL.md: {message}")]
    Doc { skill: String, message: String },

    #[error("скилл \"{skill}\": {message}")]
    Args { skill: String, message: String },

    #[error("каталог скиллов {}: {message}", path.display())]
    Registry { path: PathBuf, message: String },

    #[error("неизвестный скилл \"{0}\"")]
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

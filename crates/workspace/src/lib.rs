//! Рабочие копии: кэш bare-клонов, worktree на джобу и разбор diff.

pub mod diff;
pub mod git;
pub mod repo;
pub mod worktree;

pub use diff::{DiffIndex, DiffLine, FileDiff, FileStatus, Hunk, LineKind, parse_unified_diff};
pub use git::{Git, GitOutput};
pub use repo::RepoCache;
pub use worktree::Worktree;

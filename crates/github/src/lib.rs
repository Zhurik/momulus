//! The GitHub platform layer: a polling Trigger and an octocrab Publisher.

pub mod app;
pub mod auth;
pub mod backoff;
pub mod error;
pub mod models;
pub mod publisher;
pub mod trigger;

pub use app::{ClientProvider, FixedClient, GithubApp, StaticGitAccess, pr_refspecs};
pub use auth::AppAuth;
pub use publisher::GithubPublisher;
pub use trigger::{ClientSource, GithubTrigger, TriggerConfig};

/// The platform identifier used across the core types.
pub const PLATFORM: &str = "github";

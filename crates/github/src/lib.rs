//! Платформенный слой GitHub: Trigger на polling и Publisher на octocrab.

pub mod app;
pub mod auth;
pub mod backoff;
pub mod error;
pub mod models;
pub mod publisher;
pub mod trigger;

pub use app::{ClientProvider, FixedClient, GithubApp};
pub use auth::AppAuth;
pub use publisher::GithubPublisher;
pub use trigger::{ClientSource, GithubTrigger, TriggerConfig};

/// Идентификатор платформы во всех типах ядра.
pub const PLATFORM: &str = "github";

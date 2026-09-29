//! Инициализация tracing: человекочитаемый или JSON-формат.

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

use crate::cli::LogFormat;

/// Настраивает глобальный подписчик логов.
pub fn init(format: LogFormat, level: &str) -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .context("invalid log level")?;

    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match format {
        LogFormat::Text => builder.init(),
        LogFormat::Json => builder.json().flatten_event(true).init(),
    }
    Ok(())
}

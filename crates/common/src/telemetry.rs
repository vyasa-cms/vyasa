//! Tracing subscriber initialization and span helpers.
//!
//! [`init`] is idempotent: calling it more than once (tests, repeated
//! startups) is a no-op after the first successful installation.

use std::sync::OnceLock;

use tracing_subscriber::EnvFilter;

use crate::config::{LogFormat, VyasaConfig};

/// Initializes the global tracing subscriber.
///
/// `level` is a filter directive (e.g. `"info"`, `"vyasa=debug"`);
/// invalid directives fall back to `info`. Subsequent calls are no-ops.
pub fn init(level: &str, format: LogFormat) {
    static INITIALIZED: OnceLock<()> = OnceLock::new();
    if INITIALIZED.set(()).is_err() {
        return;
    }
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    let result = match format {
        LogFormat::Pretty => tracing_subscriber::fmt().with_env_filter(filter).try_init(),
        LogFormat::Json => tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .try_init(),
    };
    if let Err(err) = result {
        // Another subscriber may already be installed (e.g. a test
        // harness); that is not fatal, so degrade to stderr notice only.
        eprintln!("vyasa telemetry: could not install subscriber: {err}");
    }
}

/// Initializes telemetry using `config`'s log settings.
pub fn init_from_config(config: &VyasaConfig) {
    init(&config.log.level, config.log.format);
}

/// Creates an `info`-level span for a named operation, keeping span naming
/// consistent across crates.
#[must_use]
pub fn operation_span(name: &'static str) -> tracing::Span {
    tracing::info_span!("operation", name)
}

#[cfg(test)]
mod tests {
    use super::init;
    use crate::config::LogFormat;

    #[test]
    fn init_is_idempotent_and_tolerates_bad_levels() {
        init("info", LogFormat::Pretty);
        init("debug", LogFormat::Json); // no-op
        init("not a valid filter directive!!", LogFormat::Json); // falls back
    }

    #[test]
    fn operation_span_is_usable() {
        let span = crate::telemetry::operation_span("unit-test");
        let _guard = span.enter();
        tracing::info!("inside an operation span");
    }
}

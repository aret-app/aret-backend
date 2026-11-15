//! Logging setup.
//!
//! - Level: `RUST_LOG` (default `info`), e.g. `RUST_LOG=debug` or `RUST_LOG=aret_tracks=debug,info`.
//! - Format: human-readable; JSON when `ARET_LOG_FORMAT=json` (containers, for log collectors).
//!
//! Never log personal data such as emails, tokens or exact locations (spec §12).

use tracing_subscriber::{EnvFilter, fmt};

/// Installs the global logger. Call once, at the start of `main`.
pub fn init() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let json_format: bool = std::env::var("ARET_LOG_FORMAT").is_ok_and(|format| format == "json");

    let registry = tracing_subscriber::registry().with(filter);
    if json_format {
        registry.with(fmt::layer().json()).init();
    } else {
        registry.with(fmt::layer()).init();
    }
}

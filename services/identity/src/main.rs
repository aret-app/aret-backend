//! identity-svc — thin adapter for Ory Kratos (ADR-0008).
//!
//! Skeleton: only `GET /health`. Kratos webhooks → events and account deletion come in
//! aret-backend#3.

use aret_common::config::ServiceConfig;
use aret_common::{server, telemetry};

/// Port used when `ARET_HTTP_ADDR` is not set.
const DEFAULT_PORT: u16 = 8081;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::init();
    let config = ServiceConfig::try_from_env("identity", env!("CARGO_PKG_VERSION"), DEFAULT_PORT)?;

    let app = server::base_router(&config);

    server::serve(app, &config).await?;
    Ok(())
}

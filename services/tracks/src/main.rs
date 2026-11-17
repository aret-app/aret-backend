//! tracks-svc binary: configuration, database, migrations, then serve (see the library docs).

use std::sync::Arc;

use aret_auth::JwtVerifier;
use aret_common::config::ServiceConfig;
use aret_common::{server, telemetry};
use aret_tracks::{AppState, db};

/// Port used when `ARET_HTTP_ADDR` is not set.
const DEFAULT_PORT: u16 = 8082;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::init();
    let config = ServiceConfig::try_from_env("tracks", env!("CARGO_PKG_VERSION"), DEFAULT_PORT)?;
    let verifier = Arc::new(JwtVerifier::try_from_env()?);

    let database_url: String = std::env::var(db::DATABASE_URL_ENV)
        .map_err(|_| "DATABASE_URL is not set (see README: Local stack)")?;
    let pool = db::connect(&database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!("database migrations applied");

    let app = server::base_router(&config).merge(aret_tracks::router(AppState { pool, verifier }));
    server::serve(app, &config).await?;
    Ok(())
}

//! tracks-svc — recorded tracks and sync.
//!
//! Skeleton: `GET /health` plus one protected route that shows how login is required. Real
//! endpoints with PostGIS come in aret-backend#4.

use aret_auth::AuthenticatedUser;
use aret_common::config::ServiceConfig;
use aret_common::{server, telemetry};
use axum::Json;
use axum::routing::get;

/// Port used when `ARET_HTTP_ADDR` is not set.
const DEFAULT_PORT: u16 = 8082;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::init();
    let config = ServiceConfig::try_from_env("tracks", env!("CARGO_PKG_VERSION"), DEFAULT_PORT)?;

    let app = server::base_router(&config).route("/v1/tracks", get(list_tracks));

    server::serve(app, &config).await?;
    Ok(())
}

/// `GET /v1/tracks`: the signed-in user's tracks, empty until aret-backend#4.
///
/// Taking an [`AuthenticatedUser`] makes axum answer 401 to requests without a valid login.
async fn list_tracks(_user: AuthenticatedUser) -> Json<Vec<serde_json::Value>> {
    Json(Vec::new())
}

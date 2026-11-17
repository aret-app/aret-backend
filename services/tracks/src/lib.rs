//! tracks-svc — recorded tracks (aret-backend#4).
//!
//! JSON API, every route requires a Kratos session JWT (`aret-auth`):
//! - `POST /v1/tracks`: save a track; idempotent on its client-generated id
//! - `GET /v1/tracks`: the user's tracks, newest first, with cursor pagination
//! - `GET /v1/tracks/{id}`: one track with all its points
//!
//! Modules: [`model`] (API types and validation), [`stats`] (elevation gain), [`store`] (SQL),
//! [`routes`] (handlers), [`db`] (pool and migrations).

pub mod db;
pub mod model;
pub mod routes;
pub mod stats;
pub mod store;

use std::sync::Arc;

use aret_auth::JwtVerifier;
use axum::Router;
use axum::extract::{DefaultBodyLimit, FromRef};
use axum::routing::get;
use sqlx::PgPool;

/// Largest accepted request body: room for a long hike recorded every second.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Shared state of the tracks API: one database pool and one token verifier for all requests.
#[derive(Clone)]
pub struct AppState {
    /// PostgreSQL connection pool (role `tracks`).
    pub pool: PgPool,
    /// Verifies Kratos session JWTs.
    pub verifier: Arc<JwtVerifier>,
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

impl FromRef<AppState> for Arc<JwtVerifier> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.verifier)
    }
}

/// The tracks API routes, ready to merge into the service's base router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/v1/tracks",
            get(routes::list_tracks).post(routes::create_track),
        )
        .route("/v1/tracks/{id}", get(routes::get_track))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

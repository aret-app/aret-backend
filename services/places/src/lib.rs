//! places-svc — place and route search from our own copy of OpenStreetMap (ADR-0017,
//! aret-backend#24).
//!
//! aret-maps builds a catalogue of places and pre-built routes every week and publishes it
//! (`https://aret-tiles.pages.dev/catalogue/`); [`catalogue`] loads each new build into PostGIS and
//! [`api`] answers searches from it. No search ever calls a third-party service.
//!
//! [`db`]: pool and migrations (schema `places`).

pub mod api;
pub mod catalogue;
pub mod db;
pub mod detail;
pub mod network;
pub mod tours;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::get;
use sqlx::PgPool;

/// Shared state of the search API.
#[derive(Clone)]
pub struct AppState {
    /// PostgreSQL pool (role `places`).
    pub pool: PgPool,
    /// The trail network for tours, built once per catalogue.
    pub tours: Arc<api::TourCache>,
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

impl FromRef<AppState> for Arc<api::TourCache> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.tours)
    }
}

impl AppState {
    /// State with an empty tour cache.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            tours: Arc::new(api::TourCache::default()),
        }
    }
}

/// The search API routes, ready to merge into the service's base router. Public: no login.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/places", get(api::search_places))
        .route("/v1/routes", get(api::routes_near))
        .route("/v1/routes/{osm_id}", get(api::route_detail))
        .route("/v1/routes/{osm_id}/gpx", get(api::route_gpx))
        .route("/v1/tours", get(api::tours_to))
        .route("/v1/tours/detail", get(api::tour_detail))
        .route("/v1/tours/gpx", get(api::tour_gpx))
        .with_state(state)
}

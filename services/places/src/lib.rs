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

use axum::Router;
use axum::routing::get;
use sqlx::PgPool;

/// The search API routes, ready to merge into the service's base router. Public: no login.
pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/v1/places", get(api::search_places))
        .route("/v1/routes", get(api::routes_near))
        .route("/v1/routes/{osm_id}", get(api::route_detail))
        .route("/v1/routes/{osm_id}/gpx", get(api::route_gpx))
        .with_state(pool)
}

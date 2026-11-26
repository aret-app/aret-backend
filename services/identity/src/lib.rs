//! identity-svc — thin adapter for Ory Kratos (ADR-0008).
//!
//! Announces new accounts as `user.created` events (aret-backend#12):
//! - [`webhook`]: Kratos calls `POST /internal/kratos/registration` after each sign-up
//! - [`kratos`]: a catch-up loop lists Kratos identities to find any the web hook missed
//! - [`users`]: records each account once and queues its event in the outbox
//! - the outbox relay (`aret_events::outbox`) publishes the events to NATS JetStream
//!
//! [`db`]: pool and migrations (schema `identity`).

pub mod db;
pub mod kratos;
pub mod users;
pub mod webhook;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::post;
use sqlx::PgPool;

use crate::webhook::WebhookSecret;

/// Shared state of the identity API.
#[derive(Clone)]
pub struct AppState {
    /// PostgreSQL connection pool (role `identity`).
    pub pool: PgPool,
    /// Secret Kratos sends with its web hook calls.
    pub webhook_secret: WebhookSecret,
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

impl FromRef<AppState> for WebhookSecret {
    fn from_ref(state: &AppState) -> Self {
        state.webhook_secret.clone()
    }
}

/// The identity API routes, ready to merge into the service's base router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/internal/kratos/registration", post(webhook::registration))
        .with_state(state)
}

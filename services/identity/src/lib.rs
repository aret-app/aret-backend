//! identity-svc — thin adapter for Ory Kratos (ADR-0008).
//!
//! - `user.created` (aret-backend#12): [`webhook`] receives Kratos's after-registration call,
//!   [`kratos`] runs a catch-up loop that finds any the web hook missed, [`users`] records each
//!   account once and queues its event in the outbox
//! - account deletion (aret-backend#13): [`account`] serves `DELETE /v1/account`, queues
//!   `user.deleted` and removes the identity from Kratos; [`users`] tracks the services'
//!   `purge.completed` confirmations
//! - the outbox relay (`aret_events::outbox`) publishes the events to NATS JetStream
//!
//! [`db`]: pool and migrations (schema `identity`).

pub mod account;
pub mod db;
pub mod kratos;
pub mod users;
pub mod webhook;

use std::sync::Arc;

use aret_auth::JwtVerifier;
use axum::Router;
use axum::extract::FromRef;
use axum::routing::{delete, post};
use sqlx::PgPool;

use crate::kratos::KratosAdmin;
use crate::webhook::WebhookSecret;

/// Shared state of the identity API.
#[derive(Clone)]
pub struct AppState {
    /// PostgreSQL connection pool (role `identity`).
    pub pool: PgPool,
    /// Secret Kratos sends with its web hook calls.
    pub webhook_secret: WebhookSecret,
    /// Verifies Kratos session JWTs.
    pub verifier: Arc<JwtVerifier>,
    /// Kratos admin API.
    pub kratos: KratosAdmin,
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

impl FromRef<AppState> for Arc<JwtVerifier> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.verifier)
    }
}

impl FromRef<AppState> for KratosAdmin {
    fn from_ref(state: &AppState) -> Self {
        state.kratos.clone()
    }
}

/// The identity API routes, ready to merge into the service's base router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/internal/kratos/registration", post(webhook::registration))
        .route("/v1/account", delete(account::delete_account))
        .with_state(state)
}

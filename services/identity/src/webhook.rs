//! Kratos web hooks. Internal: the gateway never routes `/internal/*` (dev/gateway/Caddyfile).
//!
//! Kratos calls `POST /internal/kratos/registration` after saving a new account, without waiting
//! for the answer (`response.ignore`), so sign-ups never fail because of identity-svc. If a call
//! is lost, the catch-up loop ([`crate::kratos`]) finds the account later.

use aret_common::error::ApiError;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::users;

/// Header carrying the shared secret (Kratos `auth.type: api_key`).
pub const SECRET_HEADER: &str = "x-aret-webhook-secret";
/// Environment variable with the shared secret.
pub const SECRET_ENV: &str = "ARET_KRATOS_WEBHOOK_SECRET";
/// Shortest accepted secret; `scripts/dev-env.sh` generates 64 hex characters.
const MIN_SECRET_LEN: usize = 32;

/// The secret Kratos sends with every web hook call.
#[derive(Clone)]
pub struct WebhookSecret(String);

/// The web hook secret is missing or too short.
#[must_use]
#[derive(Debug, thiserror::Error)]
#[error(
    "{SECRET_ENV} must be set to at least {MIN_SECRET_LEN} characters (run scripts/dev-env.sh)"
)]
pub struct WebhookSecretError;

impl WebhookSecret {
    /// Wraps `secret`, which must be at least 32 characters long.
    pub fn try_new(secret: &str) -> Result<Self, WebhookSecretError> {
        if secret.len() < MIN_SECRET_LEN {
            return Err(WebhookSecretError);
        }
        Ok(Self(secret.to_owned()))
    }

    /// Reads the secret from `ARET_KRATOS_WEBHOOK_SECRET`.
    pub fn try_from_env() -> Result<Self, WebhookSecretError> {
        let secret: String = std::env::var(SECRET_ENV).map_err(|_| WebhookSecretError)?;
        Self::try_new(&secret)
    }

    /// Whether `headers` carry this secret; compares in constant time.
    fn accepts(&self, headers: &HeaderMap) -> bool {
        use subtle::ConstantTimeEq;

        headers
            .get(SECRET_HEADER)
            .is_some_and(|value| bool::from(value.as_bytes().ct_eq(self.0.as_bytes())))
    }
}

/// Body sent by `dev/kratos/webhook-registration.jsonnet`.
#[derive(Debug, Deserialize)]
struct RegistrationHook {
    identity_id: Uuid,
}

/// `POST /internal/kratos/registration`: records the new account and queues `user.created`.
/// The secret is checked before the body is read, so callers without it learn nothing.
pub async fn registration(
    State(secret): State<WebhookSecret>,
    State(pool): State<PgPool>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    if !secret.accepts(&headers) {
        return Err(ApiError::unauthorized("missing or wrong web hook secret"));
    }
    let hook: RegistrationHook = serde_json::from_slice(&body)
        .map_err(|_| ApiError::bad_request("expected {\"identity_id\": \"<uuid>\"}"))?;
    users::record(&pool, &[hook.identity_id])
        .await
        .map_err(|error| {
            tracing::error!(%error, "recording a registration failed");
            ApiError::internal()
        })?;
    Ok(StatusCode::NO_CONTENT)
}

//! `DELETE /v1/account`: the user deletes their account and all their data (GDPR Art. 17,
//! aret-backend#13).
//!
//! 1. Requires a login from the last 15 minutes, so a stolen or forgotten-open session can't do it.
//! 2. Marks the account deleted and queues `user.deleted`, in one transaction.
//! 3. Deletes the identity from Kratos: email, password and every session go at once, so nobody
//!    can sign in again. If Kratos is unreachable, the catch-up loop retries.
//! 4. Every service holding user data purges it when `user.deleted` arrives and answers with
//!    `purge.completed`; the catch-up loop warns about missing confirmations.
//!
//! Answers 202 Accepted: the services' purges run after the response. Repeating the call is
//! harmless.

use aret_auth::AuthenticatedUser;
use aret_common::error::ApiError;
use axum::extract::State;
use axum::http::StatusCode;
use sqlx::PgPool;
use time::Duration;
use uuid::Uuid;

use crate::kratos::KratosAdmin;
use crate::users;

/// How recent the login must be; same as Kratos's `privileged_session_max_age`.
pub const FRESH_LOGIN_MAX_AGE: Duration = Duration::minutes(15);

/// `DELETE /v1/account`.
pub async fn delete_account(
    State(pool): State<PgPool>,
    State(kratos): State<KratosAdmin>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let user_id = Uuid::parse_str(&user.id)
        .map_err(|_| ApiError::unauthorized("token subject is not a user id"))?;
    if !user.has_recent_login(FRESH_LOGIN_MAX_AGE) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "log in again: deleting your account needs a login from the last 15 minutes",
        ));
    }

    let newly_deleted = users::mark_deleted(&pool, user_id).await.map_err(|error| {
        tracing::error!(%error, "marking an account deleted failed");
        ApiError::internal()
    })?;
    if newly_deleted {
        tracing::info!(%user_id, "account deletion requested");
    }

    // Best effort now; the catch-up loop retries until it succeeds
    match kratos.delete_identity(user_id).await {
        Ok(()) => {
            if let Err(error) = users::mark_removed_from_kratos(&pool, user_id).await {
                tracing::warn!(%error, %user_id, "recording the Kratos deletion failed; will retry");
            }
        }
        Err(error) => {
            tracing::warn!(%error, %user_id, "deleting the identity from Kratos failed; will retry");
        }
    }
    Ok(StatusCode::ACCEPTED)
}

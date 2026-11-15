//! Request authentication for Aret services — ADR-0008 (self-hosted Ory Kratos).
//!
//! Add [`AuthenticatedUser`] as a handler argument to require a signed-in user:
//!
//! ```ignore
//! async fn list_tracks(user: AuthenticatedUser) -> Json<Vec<Track>> {
//!     // `user.id` is the Kratos identity id.
//! }
//! ```
//!
//! **Placeholder:** verifying Kratos session JWTs (JWKS) is aret-backend#3. Until then, requests
//! without a token get 401 and every token is rejected.

use aret_common::error::ApiError;
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

/// The signed-in user making the request.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticatedUser {
    /// Kratos identity id.
    pub id: String,
}

impl<State: Send + Sync> FromRequestParts<State> for AuthenticatedUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &State,
    ) -> Result<Self, Self::Rejection> {
        let Some(token) = bearer_token(parts) else {
            return Err(ApiError::unauthorized("missing bearer token"));
        };
        verify(token)
    }
}

/// The token from an `Authorization: Bearer <token>` header, if present and non-empty.
fn bearer_token(parts: &Parts) -> Option<&str> {
    let value: &str = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

/// Checks a session token and returns its user.
///
/// TODO(aret-backend#3): verify the Kratos session JWT (signature via JWKS, expiry, issuer).
fn verify(_token: &str) -> Result<AuthenticatedUser, ApiError> {
    Err(ApiError::unauthorized(
        "token verification is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;

    async fn me(user: AuthenticatedUser) -> String {
        user.id
    }

    async fn status_for(request: Request<Body>) -> StatusCode {
        use tower::ServiceExt;

        let router = Router::new().route("/me", get(me));
        router.oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn rejects_requests_without_a_token() {
        let request = Request::get("/me").body(Body::empty()).unwrap();
        assert_eq!(status_for(request).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rejects_tokens_until_verification_exists() {
        let request = Request::get("/me")
            .header(AUTHORIZATION, "Bearer some-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_for(request).await, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn reads_the_bearer_token() {
        let request = Request::get("/")
            .header(AUTHORIZATION, "Bearer abc")
            .body(())
            .unwrap();
        let (parts, ()) = request.into_parts();
        assert_eq!(bearer_token(&parts), Some("abc"));
    }
}

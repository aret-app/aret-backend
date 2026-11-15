//! API errors, returned to clients as RFC 9457 problem details (`application/problem+json`):
//!
//! ```json
//! { "type": "about:blank", "title": "Unauthorized", "status": 401, "detail": "missing bearer token" }
//! ```

use std::fmt;

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// An error returned by a handler; axum sends it as a problem+json response.
#[must_use]
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    status: StatusCode,
    /// Safe, human-readable explanation for the client. Never put internal details here.
    detail: Option<String>,
}

impl ApiError {
    /// An error with the given status and a client-facing explanation.
    pub fn new(status: StatusCode, detail: &str) -> Self {
        Self {
            status,
            detail: Some(detail.to_owned()),
        }
    }

    /// 400: the request is malformed.
    pub fn bad_request(detail: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, detail)
    }

    /// 401: the request lacks a valid login.
    pub fn unauthorized(detail: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, detail)
    }

    /// 404: the resource doesn't exist, or the user may not see it.
    pub fn not_found(detail: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, detail)
    }

    /// 500: unexpected failure. Log the cause where it happens; the client only sees the status.
    pub fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail: None,
        }
    }

    /// The HTTP status sent to the client.
    pub fn status(&self) -> StatusCode {
        self.status
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "{}: {detail}", self.status),
            None => write!(f, "{}", self.status),
        }
    }
}

impl std::error::Error for ApiError {}

/// Wire format of RFC 9457 problem details.
#[derive(Serialize)]
struct Problem<'detail> {
    #[serde(rename = "type")]
    kind: &'static str,
    title: &'static str,
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'detail str>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Problem {
            kind: "about:blank",
            title: self.status.canonical_reason().unwrap_or("Error"),
            status: self.status.as_u16(),
            detail: self.detail.as_deref(),
        };
        let mut response: Response = (self.status, Json(body)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn renders_problem_json() {
        let response = ApiError::unauthorized("missing bearer token").into_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "about:blank",
                "title": "Unauthorized",
                "status": 401,
                "detail": "missing bearer token"
            })
        );
    }

    #[test]
    fn internal_errors_hide_details() {
        assert_eq!(
            ApiError::internal().to_string(),
            "500 Internal Server Error"
        );
    }
}

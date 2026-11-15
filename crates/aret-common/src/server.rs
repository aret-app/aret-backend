//! HTTP server plumbing shared by every service.

use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::net::TcpListener;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::config::ServiceConfig;

/// Body of `GET /health`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Health {
    /// Always `"ok"` when the service answers.
    pub status: &'static str,
    /// Service name, e.g. `"tracks"`.
    pub service: &'static str,
    /// Service version, e.g. `"0.1.0"`.
    pub version: &'static str,
}

/// The router every service starts from, with `GET /health`. Add routes with `.route(...)`.
pub fn base_router(config: &ServiceConfig) -> Router {
    let health = Health {
        status: "ok",
        service: config.name,
        version: config.version,
    };
    Router::new().route("/health", get(move || async move { Json(health) }))
}

/// Serves `router` on the configured address, logging each request, until Ctrl-C or SIGTERM
/// (the signal Docker and Kubernetes send to stop a container).
pub async fn serve(router: Router, config: &ServiceConfig) -> std::io::Result<()> {
    let app: Router = router.layer(
        TraceLayer::new_for_http()
            .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
            .on_response(DefaultOnResponse::new().level(Level::INFO)),
    );

    let listener = TcpListener::bind(config.http_addr).await?;
    tracing::info!(
        service = config.name,
        version = config.version,
        address = %config.http_addr,
        "listening"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
}

/// Completes on Ctrl-C or, on Unix, SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        // If the handler can't be installed, keep serving; SIGTERM still stops the server.
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        let kind = tokio::signal::unix::SignalKind::terminate();
        match tokio::signal::unix::signal(kind) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};

    #[tokio::test]
    async fn health_reports_service_and_version() {
        use tower::ServiceExt;

        let config = ServiceConfig::try_new("tracks", "1.2.3", 8082, None).unwrap();
        let request = Request::get("/health").body(Body::empty()).unwrap();

        let response = base_router(&config).oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "status": "ok", "service": "tracks", "version": "1.2.3" })
        );
    }
}

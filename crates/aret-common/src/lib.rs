//! Shared building blocks for Aret services.
//!
//! - [`config`]: service settings from environment variables
//! - [`error`]: API errors as RFC 9457 `application/problem+json`
//! - [`telemetry`]: logging setup
//! - [`server`]: base router with `GET /health`, request logging, graceful shutdown

pub mod config;
pub mod error;
pub mod server;
pub mod telemetry;

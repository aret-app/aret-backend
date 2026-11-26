//! identity-svc binary: configuration, database, NATS, background loops, then serve (see the
//! library docs).

use std::sync::Arc;
use std::time::Duration;

use aret_auth::JwtVerifier;
use aret_common::config::ServiceConfig;
use aret_common::{server, telemetry};
use aret_events::jetstream::{self, JetStreamPublisher};
use aret_events::{consumer, outbox};
use aret_identity::kratos::{self, KratosAdmin};
use aret_identity::webhook::WebhookSecret;
use aret_identity::{AppState, db, users};

/// Port used when `ARET_HTTP_ADDR` is not set.
const DEFAULT_PORT: u16 = 8081;
/// How often the relay checks the outbox when it is empty.
const RELAY_IDLE_INTERVAL: Duration = Duration::from_secs(1);
/// Durable JetStream consumer collecting the services' purge confirmations.
const PURGE_CONSUMER: &str = "identity-purges";
/// How often the catch-up loop lists Kratos identities.
const CATCH_UP_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::init();
    let config = ServiceConfig::try_from_env("identity", env!("CARGO_PKG_VERSION"), DEFAULT_PORT)?;
    let webhook_secret = WebhookSecret::try_from_env()?;
    let verifier = Arc::new(JwtVerifier::try_from_env()?);
    let database_url: String = required_env(db::DATABASE_URL_ENV)?;
    let nats_url: String = required_env(jetstream::NATS_URL_ENV)?;
    let kratos_admin = KratosAdmin::try_new(&required_env(kratos::KRATOS_ADMIN_URL_ENV)?)?;

    let pool = db::connect(&database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!("database migrations applied");
    let publisher = JetStreamPublisher::try_connect(&nats_url).await?;
    tracing::info!("connected to NATS JetStream");

    let events = publisher.context().clone();
    tokio::spawn(outbox::run_relay(
        pool.clone(),
        publisher,
        RELAY_IDLE_INTERVAL,
    ));
    tokio::spawn(kratos::run_catch_up(
        pool.clone(),
        kratos_admin.clone(),
        CATCH_UP_INTERVAL,
    ));
    let consumer_pool = pool.clone();
    tokio::spawn(async move {
        consumer::run(events, PURGE_CONSUMER, "aret.purge.completed", |envelope| {
            users::handle_purge_event(&consumer_pool, envelope)
        })
        .await;
    });

    let app = server::base_router(&config).merge(aret_identity::router(AppState {
        pool,
        webhook_secret,
        verifier,
        kratos: kratos_admin,
    }));
    server::serve(app, &config).await?;
    Ok(())
}

/// The value of environment variable `name`, or an error naming it.
fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set (see README: Local stack)"))
}

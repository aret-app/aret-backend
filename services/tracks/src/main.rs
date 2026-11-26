//! tracks-svc binary: configuration, database, migrations, NATS, background tasks, then serve (see
//! the library docs).

use std::sync::Arc;
use std::time::Duration;

use aret_auth::JwtVerifier;
use aret_common::config::ServiceConfig;
use aret_common::{server, telemetry};
use aret_events::jetstream::{self, JetStreamPublisher};
use aret_events::{consumer, outbox};
use aret_tracks::{AppState, db, store};

/// Port used when `ARET_HTTP_ADDR` is not set.
const DEFAULT_PORT: u16 = 8082;
/// How often the relay checks the outbox when it is empty.
const RELAY_IDLE_INTERVAL: Duration = Duration::from_secs(1);
/// Durable JetStream consumer that purges deleted users' tracks.
const PURGE_CONSUMER: &str = "tracks-purge";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::init();
    let config = ServiceConfig::try_from_env("tracks", env!("CARGO_PKG_VERSION"), DEFAULT_PORT)?;
    let verifier = Arc::new(JwtVerifier::try_from_env()?);

    let database_url: String = required_env(db::DATABASE_URL_ENV)?;
    let nats_url: String = required_env(jetstream::NATS_URL_ENV)?;
    let pool = db::connect(&database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!("database migrations applied");
    let publisher = JetStreamPublisher::try_connect(&nats_url).await?;
    tracing::info!("connected to NATS JetStream");

    // Account deletion (aret-backend#13): purge on user.deleted, confirm with purge.completed
    let events = publisher.context().clone();
    tokio::spawn(outbox::run_relay(
        pool.clone(),
        publisher,
        RELAY_IDLE_INTERVAL,
    ));
    let consumer_pool = pool.clone();
    tokio::spawn(async move {
        consumer::run(events, PURGE_CONSUMER, "aret.user.deleted", |envelope| {
            store::handle_user_event(&consumer_pool, envelope)
        })
        .await;
    });

    let app = server::base_router(&config).merge(aret_tracks::router(AppState { pool, verifier }));
    server::serve(app, &config).await?;
    Ok(())
}

/// The value of environment variable `name`, or an error naming it.
fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set (see README: Local stack)"))
}

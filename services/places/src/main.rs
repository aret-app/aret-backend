//! places-svc binary: configuration, database, migrations, catalogue updates, then serve (see the
//! library docs).

use std::time::Duration;

use aret_common::config::ServiceConfig;
use aret_common::{server, telemetry};
use aret_places::catalogue::{self, Source};
use aret_places::db;

/// Port used when `ARET_HTTP_ADDR` is not set.
const DEFAULT_PORT: u16 = 8083;
/// How often to look for a new catalogue: aret-maps publishes weekly, so a few hours is plenty.
const UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::init();
    let config = ServiceConfig::try_from_env("places", env!("CARGO_PKG_VERSION"), DEFAULT_PORT)?;
    let database_url: String = std::env::var(db::DATABASE_URL_ENV)
        .map_err(|_| "DATABASE_URL is not set (see README: Local stack)")?;
    let location = std::env::var(catalogue::CATALOGUE_ENV)
        .unwrap_or_else(|_| catalogue::DEFAULT_CATALOGUE.to_owned());
    let source = Source::parse(&location)?;

    let pool = db::connect(&database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!(catalogue = %location, "database migrations applied");

    // Serve at once; searches answer 503 until the first catalogue is loaded
    tokio::spawn(catalogue::run_updates(
        pool.clone(),
        source,
        UPDATE_INTERVAL,
    ));

    // Build the tours' trail network as soon as a catalogue is there, not on a user's request
    let state = aret_places::AppState::new(pool.clone());
    let tours = std::sync::Arc::clone(&state.tours);
    tokio::spawn(async move { tours.keep_warm(pool, Duration::from_secs(15)).await });

    let app = server::base_router(&config).merge(aret_places::router(state));
    server::serve(app, &config).await?;
    Ok(())
}

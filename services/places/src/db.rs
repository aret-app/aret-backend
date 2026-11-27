//! Database connection pool and migrations.

use std::time::Duration;

use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;

/// Environment variable with the connection URL, e.g. `postgres://places:…@postgres:5432/aret`.
pub const DATABASE_URL_ENV: &str = "DATABASE_URL";

/// Opens a connection pool to `url`.
pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await
}

/// Migrations from `services/places/migrations/`, embedded at build time.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Applies pending migrations.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}

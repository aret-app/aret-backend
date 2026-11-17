//! Database connection pool and migrations.

use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// Environment variable with the connection URL, e.g. `postgres://tracks:…@postgres:5432/aret`.
pub const DATABASE_URL_ENV: &str = "DATABASE_URL";

/// Opens a connection pool to `url`.
pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await
}

/// Applies pending migrations from `services/tracks/migrations/` (embedded at build time).
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

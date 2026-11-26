//! Integration-test support: a fresh PostGIS database per test.
//!
//! Database server, in order of preference:
//! 1. `TEST_DATABASE_URL` (CI sets it, pointing at a PostGIS service container)
//! 2. the local Compose stack: `postgres://postgres:$POSTGRES_PASSWORD@127.0.0.1:5432/postgres`,
//!    with the password read from the repo's private `.env`
//!
//! Without either, DB tests print "skipped" and pass, unless `REQUIRE_DB` is set (CI), which makes
//! a missing database an error.

use sqlx::{AssertSqlSafe, Connection, PgConnection, PgPool};
use uuid::Uuid;

/// A throwaway database with PostGIS and the tracks migrations applied.
pub struct TestDb {
    /// Pool connected to the throwaway database.
    pub pool: PgPool,
    server_url: String,
    name: String,
}

impl TestDb {
    /// Creates a fresh database, or `None` (test skipped) when no database server is configured
    /// or reachable (e.g. the Compose stack is stopped). With `REQUIRE_DB` set, both are errors.
    pub async fn try_create() -> Option<Self> {
        let server_url = server_url()?;
        let name = format!("aret_test_{}", Uuid::new_v4().simple());

        let mut admin = match PgConnection::connect(&server_url).await {
            Ok(connection) => connection,
            Err(error) => {
                assert!(
                    std::env::var("REQUIRE_DB").is_err(),
                    "REQUIRE_DB is set but the test database is unreachable: {error}"
                );
                eprintln!("skipped: test database unreachable ({error}); is the stack running?");
                return None;
            }
        };
        // The name comes from a random UUID only, never from input
        sqlx::raw_sql(AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&mut admin)
            .await
            .expect("create test database");
        admin.close().await.expect("close admin connection");

        let pool = PgPool::connect(&database_url(&server_url, &name))
            .await
            .expect("connect");
        sqlx::raw_sql("CREATE EXTENSION IF NOT EXISTS postgis")
            .execute(&pool)
            .await
            .expect("enable PostGIS");
        aret_tracks::db::migrate(&pool)
            .await
            .expect("run migrations");
        Some(Self {
            pool,
            server_url,
            name,
        })
    }

    /// Drops the database. Call at the end of each test.
    pub async fn drop_database(self) {
        self.pool.close().await;
        let mut admin = PgConnection::connect(&self.server_url)
            .await
            .expect("connect");
        sqlx::raw_sql(AssertSqlSafe(format!(
            "DROP DATABASE {} WITH (FORCE)",
            self.name
        )))
        .execute(&mut admin)
        .await
        .expect("drop test database");
    }
}

/// URL of a server where the tests may create databases.
fn server_url() -> Option<String> {
    let _ = dotenvy::from_path(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.env"));
    let url = std::env::var("TEST_DATABASE_URL").ok().or_else(|| {
        let password = std::env::var("POSTGRES_PASSWORD").ok()?;
        Some(format!(
            "postgres://postgres:{password}@127.0.0.1:5432/postgres"
        ))
    });
    if url.is_none() {
        assert!(
            std::env::var("REQUIRE_DB").is_err(),
            "REQUIRE_DB is set but no TEST_DATABASE_URL: DB tests must run here"
        );
        eprintln!("skipped: no database (start the Compose stack or set TEST_DATABASE_URL)");
    }
    url
}

/// `server_url` with its database name replaced by `name`.
fn database_url(server_url: &str, name: &str) -> String {
    let base = server_url
        .rsplit_once('/')
        .map_or(server_url, |(base, _)| base);
    format!("{base}/{name}")
}

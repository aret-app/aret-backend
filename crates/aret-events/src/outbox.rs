//! Transactional outbox. Each service that publishes events has this table in its own schema
//! (copy this migration into the service):
//!
//! ```sql
//! CREATE TABLE outbox (
//!     id            uuid PRIMARY KEY,           -- Envelope id, also the JetStream message id
//!     subject       text NOT NULL,
//!     payload       jsonb NOT NULL,             -- the Envelope
//!     created_at    timestamptz NOT NULL DEFAULT now(),
//!     published_at  timestamptz                 -- NULL until the relay published it
//! );
//! CREATE INDEX outbox_unpublished_idx ON outbox (created_at, id) WHERE published_at IS NULL;
//! ```

use std::future::Future;
use std::time::Duration;

use sqlx::types::Json;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::Envelope;

/// Sends one event to the message broker; implemented by [`crate::jetstream::JetStreamPublisher`]
/// and by fakes in tests.
pub trait Publisher: Send + Sync {
    /// Publishes `payload` on `subject`; `message_id` lets the broker drop duplicates. Must only
    /// return `Ok` once the broker has stored the message.
    fn publish(
        &self,
        subject: &str,
        message_id: Uuid,
        payload: Vec<u8>,
    ) -> impl Future<Output = Result<(), PublishError>> + Send;
}

/// The broker didn't confirm a message.
#[must_use]
#[derive(Debug, thiserror::Error)]
#[error("publishing failed: {0}")]
pub struct PublishError(pub String);

/// A relay pass failed; the events stay in the outbox and are retried.
#[must_use]
#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    /// Reading or updating the outbox failed.
    #[error("outbox database error: {0}")]
    Database(#[from] sqlx::Error),
    /// The broker didn't confirm an event.
    #[error(transparent)]
    Publish(#[from] PublishError),
}

/// Stores `envelope` in the outbox, on the caller's connection or transaction, so it commits
/// together with the change it describes. Returns `false` if the same event is already there.
pub async fn enqueue(
    connection: &mut PgConnection,
    envelope: &Envelope,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO outbox (id, subject, payload, created_at) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(envelope.id)
    .bind(envelope.event.subject())
    .bind(Json(envelope))
    .bind(envelope.occurred_at)
    .execute(connection)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Publishes up to `batch` unpublished events, oldest first, and marks them published. If
/// publishing fails, nothing is marked: those events are sent again on the next pass, and the
/// broker drops the duplicates by message id.
pub async fn relay_once<P: Publisher>(
    pool: &PgPool,
    publisher: &P,
    batch: i64,
) -> Result<usize, RelayError> {
    let mut transaction = pool.begin().await?;
    // SKIP LOCKED: several relay instances can run without publishing the same row twice
    let rows: Vec<(Uuid, String, Json<serde_json::Value>)> = sqlx::query_as(
        "SELECT id, subject, payload FROM outbox WHERE published_at IS NULL \
         ORDER BY created_at, id LIMIT $1 FOR UPDATE SKIP LOCKED",
    )
    .bind(batch)
    .fetch_all(&mut *transaction)
    .await?;

    for (id, subject, Json(payload)) in &rows {
        let bytes = serde_json::to_vec(payload).expect("JSON values always serialise");
        publisher.publish(subject, *id, bytes).await?;
        sqlx::query("UPDATE outbox SET published_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(rows.len())
}

/// Runs the relay forever: publishes new events as they appear (checking every `idle_interval`
/// when there is nothing to do) and retries after errors.
pub async fn run_relay<P: Publisher>(pool: PgPool, publisher: P, idle_interval: Duration) {
    const BATCH: i64 = 100;
    loop {
        match relay_once(&pool, &publisher, BATCH).await {
            Ok(published) if published > 0 => {
                tracing::info!(published, "events published");
            }
            Ok(_) => tokio::time::sleep(idle_interval).await,
            Err(error) => {
                tracing::warn!(%error, "event relay failed; retrying");
                tokio::time::sleep(idle_interval * 5).await;
            }
        }
    }
}

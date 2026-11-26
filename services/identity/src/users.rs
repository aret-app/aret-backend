//! Accounts identity-svc knows about: `user.created` for each new one, and the deletion state of
//! deleted ones (tombstone, Kratos removal, purge confirmations).

use std::time::Duration;

use aret_events::outbox;
use aret_events::{Envelope, Event, Service};
use sqlx::PgPool;
use uuid::Uuid;

/// Records Kratos identities. For each one not seen before, stores a `user.created` event in the
/// outbox in the same transaction, so every account is announced exactly once (the relay then
/// publishes it). Returns the ids that were new.
///
/// Both the registration web hook and the catch-up loop call this; whichever comes first wins.
pub async fn record(pool: &PgPool, ids: &[Uuid]) -> Result<Vec<Uuid>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut transaction = pool.begin().await?;
    let new_ids: Vec<Uuid> = sqlx::query_scalar(
        "INSERT INTO users (id) SELECT unnest($1::uuid[]) ON CONFLICT (id) DO NOTHING RETURNING id",
    )
    .bind(ids)
    .fetch_all(&mut *transaction)
    .await?;
    for &user_id in &new_ids {
        outbox::enqueue(
            &mut transaction,
            &Envelope::new(Event::UserCreated { user_id }),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(new_ids)
}

/// Marks the account deleted and queues `user.deleted`, in one transaction. Returns `false` if it
/// was already deleted (nothing queued again). The row stays as a tombstone, so the catch-up loop
/// never announces the account again.
pub async fn mark_deleted(pool: &PgPool, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    // Inserts when the account was never recorded (deleted before the catch-up loop saw it)
    let marked: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO users (id, deleted_at) VALUES ($1, now()) \
         ON CONFLICT (id) DO UPDATE SET deleted_at = now() WHERE users.deleted_at IS NULL \
         RETURNING id",
    )
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if marked.is_some() {
        outbox::enqueue(
            &mut transaction,
            &Envelope::new(Event::UserDeleted { user_id }),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(marked.is_some())
}

/// Records that the identity is gone from Kratos.
pub async fn mark_removed_from_kratos(pool: &PgPool, user_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET kratos_deleted_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Deleted accounts still present in Kratos (an earlier Kratos call failed).
pub async fn pending_kratos_deletions(pool: &PgPool) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM users WHERE deleted_at IS NOT NULL AND kratos_deleted_at IS NULL \
         ORDER BY deleted_at",
    )
    .fetch_all(pool)
    .await
}

/// Records a service's `purge.completed`; repeats are ignored.
pub async fn record_purge(
    pool: &PgPool,
    user_id: Uuid,
    service: Service,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO purges (user_id, service) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(user_id)
        .bind(service.name())
        .execute(pool)
        .await?;
    Ok(())
}

/// A deleted account some services haven't confirmed purging yet.
#[derive(Debug, Clone, PartialEq)]
pub struct OverduePurge {
    /// Kratos identity id.
    pub user_id: Uuid,
    /// Services that haven't sent `purge.completed`.
    pub missing: Vec<String>,
}

/// Accounts deleted more than `older_than` ago that some service in
/// [`Service::ALL_WITH_USER_DATA`] hasn't confirmed purging.
pub async fn overdue_purges(
    pool: &PgPool,
    older_than: Duration,
) -> Result<Vec<OverduePurge>, sqlx::Error> {
    let expected: Vec<&str> = Service::ALL_WITH_USER_DATA
        .iter()
        .map(|s| s.name())
        .collect();
    let rows: Vec<(Uuid, Vec<String>)> = sqlx::query_as(
        "SELECT u.id, array_agg(s.name ORDER BY s.name) \
         FROM users u CROSS JOIN unnest($1::text[]) AS s(name) \
         WHERE u.deleted_at < now() - $2 * interval '1 second' \
           AND NOT EXISTS (SELECT 1 FROM purges p WHERE p.user_id = u.id AND p.service = s.name) \
         GROUP BY u.id ORDER BY u.id",
    )
    .bind(&expected)
    .bind(older_than.as_secs_f64())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, missing)| OverduePurge { user_id, missing })
        .collect())
}

/// Handles one event from the `aret.purge.completed` consumer.
pub async fn handle_purge_event(pool: &PgPool, envelope: Envelope) -> Result<(), sqlx::Error> {
    match envelope.event {
        Event::PurgeCompleted { user_id, service } => {
            tracing::info!(%user_id, service = service.name(), "purge confirmed");
            record_purge(pool, user_id, service).await
        }
        // The consumer only subscribes to purge.completed
        Event::UserCreated { .. } | Event::UserDeleted { .. } => Ok(()),
    }
}

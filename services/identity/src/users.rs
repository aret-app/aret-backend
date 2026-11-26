//! Accounts identity-svc knows about, and the `user.created` event for each new one.

use aret_events::outbox;
use aret_events::{Envelope, Event};
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

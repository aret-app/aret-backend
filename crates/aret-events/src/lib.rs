//! Events between Aret services (ADR-0009): what happened, published on NATS JetStream.
//!
//! - [`Event`] / [`Envelope`]: the event types and their wire format (JSON)
//! - [`outbox`]: the transactional outbox. A service writes the event in the same database
//!   transaction as its change; a relay publishes it afterwards, so a crash never loses an event
//! - [`jetstream`]: the NATS JetStream publisher and stream setup
//!
//! Events carry ids only, never personal data (spec §10).

pub mod jetstream;
pub mod outbox;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// JetStream stream holding every Aret event.
pub const STREAM_NAME: &str = "ARET_EVENTS";
/// Subjects captured by [`STREAM_NAME`].
pub const STREAM_SUBJECTS: &str = "aret.>";

/// Namespace for deterministic event ids (UUID v5); never change it.
const EVENT_ID_NAMESPACE: Uuid = Uuid::from_u128(0x6a3c_4f2e_8d1b_4e7a_9c5d_2b8f_1e6a_7c30);

/// Something that happened, which other services may react to.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Kratos registered an account (aret-backend#12).
    UserCreated {
        /// Kratos identity id.
        user_id: Uuid,
    },
    /// The user asked to delete their account; every service must purge their data
    /// (aret-backend#13).
    UserDeleted {
        /// Kratos identity id.
        user_id: Uuid,
    },
}

impl Event {
    /// NATS subject the event is published on.
    pub fn subject(&self) -> &'static str {
        match self {
            Self::UserCreated { .. } => "aret.user.created",
            Self::UserDeleted { .. } => "aret.user.deleted",
        }
    }

    /// Stable id: the same event about the same user always gets the same id, so duplicates
    /// (retries, repeated web hooks) are dropped by the outbox and by JetStream.
    pub fn id(&self) -> Uuid {
        let user_id = match self {
            Self::UserCreated { user_id } | Self::UserDeleted { user_id } => user_id,
        };
        let name = format!("{}:{user_id}", self.subject());
        Uuid::new_v5(&EVENT_ID_NAMESPACE, name.as_bytes())
    }
}

/// An event as stored in the outbox and published: `{"id", "occurred_at", "type", ...fields}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Event id, also the JetStream message id (deduplication).
    pub id: Uuid,
    /// When it happened.
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    /// What happened.
    #[serde(flatten)]
    pub event: Event,
}

impl Envelope {
    /// Wraps `event`, happening now.
    pub fn new(event: Event) -> Self {
        Self {
            id: event.id(),
            occurred_at: OffsetDateTime::now_utc(),
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use time::macros::datetime;

    #[test]
    fn serialises_to_flat_json() {
        let user_id = Uuid::from_u128(42);
        let envelope = Envelope {
            id: Uuid::from_u128(1),
            occurred_at: datetime!(2026-11-26 10:00 UTC),
            event: Event::UserCreated { user_id },
        };
        assert_eq!(
            serde_json::to_value(&envelope).unwrap(),
            serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "occurred_at": "2026-11-26T10:00:00Z",
                "type": "user_created",
                "user_id": "00000000-0000-0000-0000-00000000002a",
            })
        );
    }

    #[test]
    fn ids_are_stable_per_event_and_user() {
        let user = Uuid::from_u128(7);
        let created = Event::UserCreated { user_id: user };
        assert_eq!(created.id(), created.id());
        assert_ne!(created.id(), Event::UserDeleted { user_id: user }.id());
        assert_ne!(
            created.id(),
            Event::UserCreated {
                user_id: Uuid::from_u128(8)
            }
            .id()
        );
    }

    #[test]
    fn subjects_are_under_the_stream() {
        let user_id = Uuid::from_u128(7);
        for event in [
            Event::UserCreated { user_id },
            Event::UserDeleted { user_id },
        ] {
            assert!(event.subject().starts_with("aret."));
        }
    }
}

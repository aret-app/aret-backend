//! NATS JetStream: the stream that stores Aret events, and the publisher the relay uses.

use std::time::Duration;

use async_nats::jetstream::message::PublishMessage;
use async_nats::jetstream::{self, stream};
use uuid::Uuid;

use crate::outbox::{PublishError, Publisher};
use crate::{STREAM_NAME, STREAM_SUBJECTS};

/// Environment variable with the NATS address, e.g. `nats://nats:4222`.
pub const NATS_URL_ENV: &str = "NATS_URL";

/// How long JetStream remembers message ids to drop duplicates. Long enough to cover relay
/// retries after an outage of the broker or the service.
const DUPLICATE_WINDOW: Duration = Duration::from_secs(60 * 60);
/// How long events are kept: ids only, no personal data (spec §10).
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Publishes events to the `ARET_EVENTS` stream.
pub struct JetStreamPublisher {
    context: jetstream::Context,
}

impl JetStreamPublisher {
    /// Connects to NATS at `url` and creates the `ARET_EVENTS` stream if it doesn't exist yet.
    pub async fn try_connect(url: &str) -> Result<Self, PublishError> {
        let client = async_nats::connect(url)
            .await
            .map_err(|error| PublishError(format!("connecting to {url}: {error}")))?;
        let context = jetstream::new(client);
        context
            .get_or_create_stream(stream::Config {
                name: STREAM_NAME.to_owned(),
                subjects: vec![STREAM_SUBJECTS.to_owned()],
                duplicate_window: DUPLICATE_WINDOW,
                max_age: MAX_AGE,
                ..Default::default()
            })
            .await
            .map_err(|error| PublishError(format!("creating stream {STREAM_NAME}: {error}")))?;
        Ok(Self { context })
    }

    /// The JetStream context, e.g. to create consumers.
    pub fn context(&self) -> &jetstream::Context {
        &self.context
    }
}

impl Publisher for JetStreamPublisher {
    async fn publish(
        &self,
        subject: &str,
        message_id: Uuid,
        payload: Vec<u8>,
    ) -> Result<(), PublishError> {
        let message = PublishMessage::build()
            .payload(payload.into())
            .message_id(message_id.to_string());
        let acknowledgement = self
            .context
            .send_publish(subject.to_owned(), message)
            .await
            .map_err(|error| PublishError(error.to_string()))?;
        // Wait until JetStream has stored the message
        acknowledgement
            .await
            .map_err(|error| PublishError(error.to_string()))?;
        Ok(())
    }
}

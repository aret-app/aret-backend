//! Durable JetStream consumers: run a handler for every event on a subject, at least once.
//!
//! The consumer is durable: JetStream remembers which events this service has handled, so after
//! a restart it continues where it stopped, and a new consumer first catches up on everything still
//! in the stream (30 days). Handlers must be idempotent: an event is delivered again when the
//! handler fails, or when the service stops before acknowledging it.

use std::fmt::Display;
use std::future::Future;
use std::time::Duration;

use async_nats::jetstream::consumer::{AckPolicy, PullConsumer, pull};
use async_nats::jetstream::{self, AckKind};
use futures_util::StreamExt;

use crate::{Envelope, STREAM_NAME};

/// How long JetStream waits for an acknowledgement before delivering an event again.
const ACK_WAIT: Duration = Duration::from_secs(60);
/// Delay before a failed event is delivered again.
const RETRY_DELAY: Duration = Duration::from_secs(10);
/// Delay before reconnecting after the consumer itself failed (e.g. NATS restarting).
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// Runs `handler` for every event on `subject`, forever, as the durable consumer `name`.
///
/// - handler `Ok`: the event is acknowledged and never delivered again
/// - handler `Err`: logged and delivered again after a delay
/// - payload that isn't an [`Envelope`]: logged and dropped (retrying can't fix it)
pub async fn run<F, Fut, E>(context: jetstream::Context, name: &str, subject: &str, handler: F)
where
    F: Fn(Envelope) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Display,
{
    loop {
        if let Err(error) = consume(&context, name, subject, &handler).await {
            tracing::warn!(consumer = name, %error, "event consumer stopped; reconnecting");
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

/// Consumes until the message stream fails.
async fn consume<F, Fut, E>(
    context: &jetstream::Context,
    name: &str,
    subject: &str,
    handler: &F,
) -> Result<(), String>
where
    F: Fn(Envelope) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Display,
{
    let stream = context
        .get_stream(STREAM_NAME)
        .await
        .map_err(|error| format!("stream {STREAM_NAME}: {error}"))?;
    let consumer: PullConsumer = stream
        .get_or_create_consumer(
            name,
            pull::Config {
                durable_name: Some(name.to_owned()),
                filter_subject: subject.to_owned(),
                ack_policy: AckPolicy::Explicit,
                ack_wait: ACK_WAIT,
                ..Default::default()
            },
        )
        .await
        .map_err(|error| format!("consumer {name}: {error}"))?;
    let mut messages = consumer
        .messages()
        .await
        .map_err(|error| format!("subscribing: {error}"))?;
    tracing::info!(consumer = name, subject, "consuming events");

    while let Some(message) = messages.next().await {
        let message = message.map_err(|error| format!("receiving: {error}"))?;
        let acknowledgement = match serde_json::from_slice::<Envelope>(&message.payload) {
            Err(error) => {
                tracing::error!(consumer = name, %error, "dropping an event that isn't an Envelope");
                AckKind::Term
            }
            Ok(envelope) => {
                let event_id = envelope.id;
                match handler(envelope).await {
                    Ok(()) => AckKind::Ack,
                    Err(error) => {
                        tracing::warn!(consumer = name, %event_id, %error, "handling an event failed; will retry");
                        AckKind::Nak(Some(RETRY_DELAY))
                    }
                }
            }
        };
        message
            .ack_with(acknowledgement)
            .await
            .map_err(|error| format!("acknowledging: {error}"))?;
    }
    Ok(())
}

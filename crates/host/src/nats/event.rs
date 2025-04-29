//! NATS implementation of the wasmCloud [crate::wasmbus::event::EventPublisher] extension trait

use anyhow::Context;
use cloudevents::{EventBuilder, EventBuilderV10};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tracing::{instrument, warn};
use ulid::Ulid;
use uuid::Uuid;

use crate::event::EventPublisher;

/// NATS implementation of the wasmCloud [crate::wasmbus::event::EventPublisher] extension trait,
/// sending events to the NATS message bus with a CloudEvents payload envelope.
pub struct NatsEventPublisher {
    event_builder: EventBuilderV10,
    lattice: String,
    ctl_nats: async_nats::Client,
}

impl NatsEventPublisher {
    /// Create a new NATS event publisher.
    ///
    /// # Arguments
    ///
    /// * `source` - The source of the event, typically the host ID.
    /// * `lattice` - The lattice name to use for the event publisher.
    /// * `ctl_nats` - The NATS client to use for publishing events.
    pub fn new(source: String, lattice: String, ctl_nats: async_nats::Client) -> Self {
        Self {
            event_builder: EventBuilderV10::new().source(source),
            lattice,
            ctl_nats,
        }
    }
}

#[async_trait::async_trait]
impl EventPublisher for NatsEventPublisher {
    #[instrument(skip(self, data))]
    async fn publish_event(&self, name: &str, data: serde_json::Value) -> anyhow::Result<()> {
        let now = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .context("failed to format current time")?;
        let ev = self
            .event_builder
            .clone()
            .ty(format!("com.wasmcloud.lattice.{name}"))
            .id(Uuid::from_u128(Ulid::new().into()).to_string())
            .time(now)
            .data("application/json", data)
            .build()
            .context("failed to build cloud event")?;
        let ev = serde_json::to_vec(&ev).context("failed to serialize event")?;
        let max_payload = self.ctl_nats.server_info().max_payload;
        let lattice = &self.lattice;
        if ev.len() > max_payload {
            warn!(
                size = ev.len(),
                max_size = max_payload,
                event = name,
                lattice = &lattice,
                "event payload is too large to publish and may fail",
            );
        }
        self.ctl_nats
            .publish(format!("wasmbus.evt.{lattice}.{name}"), ev.into())
            .await
            .with_context(|| format!("failed to publish `{name}` event"))
    }
}

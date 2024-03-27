use std::task::Poll;

use anyhow::Result;
use chrono::{DateTime, Local};
use futures::{Stream, StreamExt};
use tracing::debug;
use wasmcloud_control_interface::ComponentId;

/// A struct that represents an invocation that was observed by the spier.
#[derive(Debug)]
pub struct ObservedInvocation {
    /// The timestamp when this was received
    pub timestamp: DateTime<Local>,
    /// The name or id of the entity that sent this invocation
    pub from: String,
    /// The name or id of the entity that received this invocation
    pub to: String,
    /// The operation that was invoked
    pub operation: String,
    /// The inner message that was received. We will attempt to parse the inner message from CBOR
    /// and JSON into a JSON string and fall back to the raw bytes if we are unable to do so
    pub message: ObservedMessage,
}

/// A inner message that we've seen in an invocation message. This will either be a raw bytes or a
/// parsed value if it was a format we recognized.
///
/// Please note that this struct is meant for debugging, so its `Display` implementation does some
/// heavier lifting like contructing strings from the raw bytes.
#[derive(Debug)]
pub enum ObservedMessage {
    Raw(Vec<u8>),
    Parsed(String),
}

impl std::fmt::Display for ObservedMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObservedMessage::Raw(bytes) => write!(f, "{}", String::from_utf8_lossy(bytes)),
            ObservedMessage::Parsed(v) => {
                write!(f, "{v}")
            }
        }
    }
}

impl ObservedMessage {
    #[must_use]
    pub fn parse(data: Vec<u8>) -> Self {
        // Try parsing with msgpack and then with cbor. If neither work, then just return the raw
        // NOTE(thomastaylor312): I don't think anyone else does their own encoding, but if that
        // becomes popular, we can add support for it here
        let mut serializer = serde_json::Serializer::pretty(Vec::new());
        let parsed = match serde_transcode::transcode(
            &mut rmp_serde::Deserializer::new(&data[..]),
            &mut serializer,
        ) {
            // SAFETY: We know that JSON writes to valid UTF-8
            Ok(()) => String::from_utf8(serializer.into_inner()).unwrap(),
            Err(_) => {
                // Reset the buffer in case we wrote some data on previous failure
                let mut serializer = serde_json::Serializer::pretty(Vec::new());
                match serde_transcode::transcode(
                    &mut serde_cbor::Deserializer::from_reader(&data[..]),
                    &mut serializer,
                ) {
                    Ok(()) => String::from_utf8(serializer.into_inner()).unwrap(),
                    Err(_) => return Self::Raw(data),
                }
            }
        };
        Self::Parsed(parsed)
    }
}

/// A struct that can spy on the RPC messages sent to and from an component, consumable as a stream
pub struct Spier {
    stream: futures::stream::SelectAll<async_nats::Subscriber>,
    component_id: ComponentId,
    friendly_name: Option<String>,
}

impl Spier {
    /// Creates a new Spier instance for the given component. Will return an error if the component cannot
    /// be found or if there are connection issues
    pub async fn new(
        component_id: &str,
        ctl_client: &wasmcloud_control_interface::Client,
        nats_client: &async_nats::Client,
    ) -> Result<Self> {
        let linked_component = get_linked_components(component_id, ctl_client).await?;

        let lattice = &ctl_client.lattice;
        let rpc_topic = format!("{lattice}.{component_id}.wrpc.>");
        let component_stream = nats_client.subscribe(rpc_topic).await?;

        let mut subs = futures::future::join_all(linked_component.iter().map(|prov| {
            let topic = format!("{lattice}.{}.wrpc.>", &prov.id);
            nats_client.subscribe(topic)
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
        subs.push(component_stream);

        let stream = futures::stream::select_all(subs);

        Ok(Self {
            stream,
            component_id: component_id.to_string(),
            friendly_name: None,
        })
    }

    /// Returns the component name, or id if no name is set, that this spier is spying on
    pub fn component_id(&self) -> &str {
        self.friendly_name
            .as_deref()
            .unwrap_or_else(|| self.component_id.as_ref())
    }
}

impl Stream for Spier {
    type Item = ObservedInvocation;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.stream.poll_next_unpin(cx) {
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(msg)) => {
                // lattice.component.wrpc.0.0.1.operation.function
                let subject_parts = msg.subject.split('.').collect::<Vec<_>>();
                let component_id = subject_parts.get(1);
                let operation = subject_parts.get(6);
                let function = subject_parts.get(7);

                if component_id.is_none() || operation.is_none() || function.is_none() {
                    debug!("Received invocation with invalid subject: {}", msg.subject);
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                let component_id = component_id.unwrap();
                let operation = format!("{}.{}", operation.unwrap(), function.unwrap());

                let (from, to) = if component_id == &self.component_id {
                    // Attempt to get the source from the message header
                    let from = msg
                        .headers
                        .and_then(|headers| headers.get("source-id").map(ToString::to_string))
                        .unwrap_or_else(|| "linked component".to_string());
                    (from, (*component_id).to_string())
                } else {
                    (self.component_id.to_string(), (*component_id).to_string())
                };

                // NOTE(thomastaylor312): Ideally we'd consume `msg.payload` above with a
                // `Cursor` and `from_reader` and then manually reconstruct the acking using the
                // message context, but I didn't want to waste time optimizing yet
                Poll::Ready(Some(ObservedInvocation {
                    timestamp: Local::now(),
                    from,
                    to,
                    operation,
                    message: ObservedMessage::parse(msg.payload.to_vec()),
                }))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Debug)]
struct ProviderDetails {
    id: ComponentId,
}

/// Fetches all components linked to the given component
async fn get_linked_components(
    component_id: &str,
    ctl_client: &wasmcloud_control_interface::Client,
) -> Result<Vec<ProviderDetails>> {
    let details = ctl_client
        .get_links()
        .await
        .map_err(|e| anyhow::anyhow!("Unable to get links: {e:?}"))
        .map(|response| response.response)?
        .map(|linkdefs| {
            linkdefs
                .into_iter()
                .filter_map(|link| {
                    if link.source_id == component_id {
                        Some(ProviderDetails { id: link.target })
                    } else if link.target == component_id {
                        Some(ProviderDetails { id: link.source_id })
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(details)
}

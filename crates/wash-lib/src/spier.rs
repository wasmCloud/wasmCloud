use std::{collections::HashMap, str::FromStr, task::Poll};

use anyhow::Result;
use chrono::{DateTime, Local};
use futures::{Stream, StreamExt};
use wasmbus_rpc::core::Invocation;

use crate::{
    common::{find_actor_id, CLAIMS_NAME, CLAIMS_SUBJECT},
    id::{ModuleId, ServiceId},
};

/// A struct that represents an invocation that was observed by the spier.
#[derive(Debug)]
pub struct ObservedInvocation {
    /// The actual invocation from the wire, but the `.msg` field will always be empty as we are
    /// consuming it to attempt to parse it.
    pub invocation: Invocation,
    /// The timestamp when this was received
    pub timestamp: DateTime<Local>,
    /// The name or id of the entity that sent this invocation
    pub from: String,
    /// The name or id of the entity that received this invocation
    pub to: String,
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
                write!(f, "{}", v)
            }
        }
    }
}

impl ObservedMessage {
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
            Ok(_) => String::from_utf8(serializer.into_inner()).unwrap(),
            Err(_) => {
                // Reset the buffer in case we wrote some data on previous failure
                let mut serializer = serde_json::Serializer::pretty(Vec::new());
                match serde_transcode::transcode(
                    &mut serde_cbor::Deserializer::from_reader(&data[..]),
                    &mut serializer,
                ) {
                    Ok(_) => String::from_utf8(serializer.into_inner()).unwrap(),
                    Err(_) => return Self::Raw(data),
                }
            }
        };
        Self::Parsed(parsed)
    }
}

/// A struct that can spy on the RPC messages sent to and from an actor, consumable as a stream
pub struct Spier {
    stream: futures::stream::SelectAll<async_nats::Subscriber>,
    actor_id: ModuleId,
    friendly_name: Option<String>,
    provider_info: HashMap<String, ProviderDetails>,
}

impl Spier {
    /// Creates a new Spier instance for the given actor. Will return an error if the actor cannot
    /// be found or if there are connection issues
    pub async fn new(
        actor_id_or_name: &str,
        ctl_client: &wasmcloud_control_interface::Client,
        nats_client: &async_nats::Client,
    ) -> Result<Self> {
        let (actor_id, friendly_name) = find_actor_id(actor_id_or_name, ctl_client).await?;
        let linked_providers = get_linked_providers(&actor_id, ctl_client).await?;

        let rpc_topic_prefix = format!("wasmbus.rpc.{}", ctl_client.lattice_prefix);
        let actor_stream = nats_client
            .subscribe(format!("{}.{}", rpc_topic_prefix, actor_id.as_ref()))
            .await?;

        let mut subs = futures::future::join_all(linked_providers.iter().map(|prov| {
            let topic = format!(
                "{}.{}.{}",
                rpc_topic_prefix,
                prov.id.as_ref(),
                &prov.link_name
            );
            nats_client.subscribe(topic)
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
        subs.push(actor_stream);

        let stream = futures::stream::select_all(subs);

        Ok(Self {
            stream,
            actor_id,
            friendly_name,
            provider_info: linked_providers
                .into_iter()
                .map(|prov| (prov.id.clone().into_string(), prov))
                .collect(),
        })
    }

    /// Returns the actor name, or id if no name is set, that this spier is spying on
    pub fn actor_id(&self) -> &str {
        self.friendly_name
            .as_deref()
            .unwrap_or_else(|| self.actor_id.as_ref())
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
                // Try to parse the invocation first
                let mut inv: Invocation = match rmp_serde::from_slice(&msg.payload) {
                    Ok(inv) => inv,
                    Err(_e) => {
                        // TODO: We should probably have some logging here
                        // Just skip it if we can't parse it. This means we need to tell the executor to automatically wake up and poll immediately if we skip
                        cx.waker().wake_by_ref();
                        return Poll::Pending;
                    }
                };
                let body = inv.msg;
                inv.msg = Vec::new();

                if inv.origin.is_provider() && inv.target.public_key() != self.actor_id.as_ref() {
                    // This is a provider invocation that isn't for us, so we should skip it
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                let from = if inv.origin.is_actor() {
                    self.friendly_name
                        .clone()
                        .unwrap_or_else(|| inv.origin.public_key())
                } else {
                    let pubkey = inv.origin.public_key();
                    self.provider_info
                        .get(&pubkey)
                        .and_then(|prov| prov.friendly_name.clone())
                        .unwrap_or(pubkey)
                };
                let to = if inv.target.is_actor() {
                    self.friendly_name
                        .clone()
                        .unwrap_or_else(|| inv.target.public_key())
                } else {
                    let pubkey = inv.target.public_key();
                    self.provider_info
                        .get(&pubkey)
                        .and_then(|prov| prov.friendly_name.clone())
                        .unwrap_or(pubkey)
                };
                // NOTE(thomastaylor312): Ideally we'd consume `msg.payload` above with a
                // `Cursor` and `from_reader` and then manually reconstruct the acking using the
                // message context, but I didn't want to waste time optimizing yet
                Poll::Ready(Some(ObservedInvocation {
                    invocation: inv,
                    timestamp: Local::now(),
                    from,
                    to,
                    message: ObservedMessage::parse(body),
                }))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Debug)]
struct ProviderDetails {
    id: ServiceId,
    link_name: String,
    friendly_name: Option<String>,
}

/// Fetches all providers linked to the given actor, along with their link names
async fn get_linked_providers(
    actor_id: &ModuleId,
    ctl_client: &wasmcloud_control_interface::Client,
) -> Result<Vec<ProviderDetails>> {
    let mut details = ctl_client
        .query_links()
        .await
        .map_err(|e| anyhow::anyhow!("Unable to get linkdefs: {e:?}"))
        .map(|linkdefs| {
            linkdefs
                .into_iter()
                .filter_map(|link| {
                    if link.actor_id == actor_id.as_ref() {
                        let provider_id = ServiceId::from_str(&link.provider_id).ok()?;
                        Some(ProviderDetails {
                            id: provider_id,
                            link_name: link.link_name,
                            friendly_name: None,
                        })
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        })?;
    let mut claim_names: HashMap<String, String> = ctl_client
        .get_claims()
        .await
        .map_err(|e| anyhow::anyhow!("Unable to get claims: {e:?}"))?
        .into_iter()
        .filter_map(|mut claims| {
            let id = claims.remove(CLAIMS_SUBJECT).unwrap_or_default();
            // If it isn't a provider, skip
            if !id.starts_with(ServiceId::prefix()) {
                return None;
            }
            // Only return it if it has a name
            claims.remove(CLAIMS_NAME).map(|name| (id, name))
        })
        .collect();
    details.iter_mut().for_each(|detail| {
        detail.friendly_name = claim_names.remove(detail.id.as_ref());
    });
    Ok(details)
}

//! # NATS Key Value Cache
//!
//! A simple distributed cache that exposes a `wasmcloud:keyvalue` capability provider
//! contract. All state changes are replicated over NATS in the form of events that are
//! then reconstituted by other nodes to build state. Replays can be requested with high
//! watermark values in order to reload/synchronize state for joining or lagging nodes.
//!
//! ## Protocol
//!
//! | Subject Pattern | Usage |
//! |---|---|
//! | {root}.events | Subscribed to by all running providers. Each event received on this subject is processed by an aggregate to update local state |
//! | {root}.replay.req | Queue subscribed by all running providers. In response to a request, a provider will stream responses to the requester |
//!
//! When a requester sends a request, they will send it with a high watermark (`hwm`). This indicates the offset from zero of the latest message
//! the requester has received. In response, the provider that handled the request will first send a [ReplayRequestAck] back on the reply
//! subject, indicating the local (handling provider) `hwm` and the `hwm` sent by the requester. Subtracting these two will tell the
//! requester how many more messages to receive on that reply subject, one for each event the requester has not received.
//!
//! As an example, assume the requester has a high watermark of 12, and the handling provider has a high watermark of 15. The requester will
//! first receive a [ReplayRequestAck] indicating how many subsequent events to expect in reply, followed then by `n` [CacheEvent]s.

mod events;
mod kvcache;

pub use events::CacheEvent;

#[macro_use]
extern crate eventsourcing_derive;

extern crate eventsourcing;

extern crate wasmcloud_actor_core as core;
extern crate wasmcloud_actor_keyvalue as keyvalue;
#[macro_use]
extern crate wasmcloud_provider_core as codec;
#[macro_use]
extern crate log;
use crossbeam_channel::{select, tick, Receiver, Sender};

use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::SYSTEM_ACTOR;
use codec::core::{OP_BIND_ACTOR, OP_HEALTH_REQUEST, OP_REMOVE_ACTOR};
use core::{CapabilityConfiguration, HealthCheckResponse};
use events::{Cache, CacheData, CacheEventWrapper};
use eventsourcing::Aggregate;
use keyvalue::{
    deserialize, serialize, AddArgs, AddResponse, ClearArgs, DelArgs, DelResponse, GetArgs,
    GetResponse, KeyExistsArgs, ListItemDeleteArgs, ListRangeResponse, ListResponse, PushArgs,
    RangeArgs, SetAddArgs, SetArgs, SetIntersectionArgs, SetOperationResponse, SetQueryArgs,
    SetQueryResponse, SetRemoveArgs, SetResponse, SetUnionArgs,
};
use kvcache::KeyValueStore;
use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, RwLock},
    thread,
    time::Duration,
};
use uuid::Uuid;
use wascap::prelude::KeyPair;

type MessageHandlerResult = Result<Vec<u8>, Box<dyn Error + Send + Sync + 'static>>;

const DEFAULT_NATS_URL: &str = "nats://0.0.0.0:4222";

#[doc(hidden)]
#[cfg(not(feature = "static_plugin"))]
capability_provider!(NatsReplicatedKVProvider, NatsReplicatedKVProvider::new);

const NATS_URL_CONFIG_KEY: &str = "NATS_URL";
const CLIENT_SEED_CONFIG_KEY: &str = "CLIENT_SEED";
const CLIENT_JWT_CONFIG_KEY: &str = "CLIENT_JWT";
const STATE_REPL_SUBJECT_KEY: &str = "STATE_REPL_SUBJECT";
const STATE_REPLAY_SUBJECT_KEY: &str = "REPLAY_REQ_SUBJECT";
const HEARTBEAT_KEY: &str = "REPLAY_HEARTBEAT_SECS";

const DEFAULT_STATE_REPL_SUBJECT: &str = "lattice.state.events";
const DEFAULT_REPLAY_REQ_SUBJECT: &str = "lattice.state.replay";
const DEFAULT_REPLAY_HEARTBEAT_SECONDS: &str = "60";

/// An instance of a `wasmcloud:keyvalue` capability provider that replicates changes to the
/// cache by means of pub/sub over a NATS message broker
#[derive(Clone)]
pub struct NatsReplicatedKVProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    cache: Arc<RwLock<CacheData>>,
    nc: Arc<RwLock<Option<nats::Connection>>>,
    event_subject: Arc<RwLock<String>>,
    id: String,
    terminator: Arc<RwLock<Option<Sender<bool>>>>,
}

impl Default for NatsReplicatedKVProvider {
    fn default() -> Self {
        if env_logger::try_init().is_err() {}
        NatsReplicatedKVProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            cache: Arc::new(RwLock::new(CacheData::new(KeyValueStore::new()))),
            nc: Arc::new(RwLock::new(None)),
            event_subject: Arc::new(RwLock::new("".to_string())),
            id: Uuid::new_v4().to_string(),
            terminator: Arc::new(RwLock::new(None)),
        }
    }
}

impl NatsReplicatedKVProvider {
    pub fn new() -> Self {
        Self::default()
    }

    fn configure(&self, config: CapabilityConfiguration) -> MessageHandlerResult {
        if config.values.contains_key(NATS_URL_CONFIG_KEY) {
            match self.initialize_connection(config.values) {
                Ok(_) => {
                    info!("KV cache configured for {}", config.module);
                    Ok(vec![])
                }
                Err(e) => {
                    error!("Failed to configure KV cache for {}: {}", config.module, e);
                    Err(e)
                }
            }
        } else {
            info!("No NATS URL present, falling back to standalone/isolated KV cache");
            Ok(vec![])
        }
    }

    fn remove_actor(&self, _config: CapabilityConfiguration) -> MessageHandlerResult {
        let mut lock = self.nc.write().unwrap();
        if let Some(nc) = lock.take() {
            nc.close();
        }
        Ok(vec![])
    }

    fn initialize_connection(
        &self,
        values: HashMap<String, String>,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let default_subject = DEFAULT_STATE_REPL_SUBJECT.to_string();
        let subject = values
            .get(STATE_REPL_SUBJECT_KEY)
            .unwrap_or(&default_subject)
            .to_string();
        *self.event_subject.write().unwrap() = subject.to_string();

        let default_replay_req_subject = DEFAULT_REPLAY_REQ_SUBJECT.to_string();
        let replay_req_subject = values
            .get(STATE_REPLAY_SUBJECT_KEY)
            .unwrap_or(&default_replay_req_subject)
            .to_string();

        let cache = self.cache.clone();
        let cache2 = self.cache.clone();
        let cache3 = self.cache.clone();
        // TODO: get authentication information from the values map
        let nc = nats_connection_from_values(values.clone())?;
        let origin = self.id.to_string();
        nc.subscribe(&subject)?.with_handler(move |msg| {
            let evt: CacheEventWrapper = deserialize(&msg.data).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Deserialization failure: {}", e),
                )
            })?;
            if evt.origin_id != origin {
                let _ = handle_state_event(&evt.event, cache.clone());
            }
            Ok(())
        });

        regenerate_cache_from_replay(&nc, cache2.clone(), &replay_req_subject)?;

        nc.queue_subscribe(&replay_req_subject, &replay_req_subject)?
            .with_handler(move |msg| process_replay_request(msg, cache3.clone()));

        let nc2 = nc.clone();
        let mut lock = self.nc.write().unwrap();
        *lock = Some(nc);

        let delay = Duration::from_secs(
            values
                .get(HEARTBEAT_KEY)
                .unwrap_or(&DEFAULT_REPLAY_HEARTBEAT_SECONDS.to_string())
                .parse()
                .unwrap(),
        );
        let c = cache2;
        let conn = nc2;

        let (term_s, term_r): (Sender<bool>, Receiver<bool>) = crossbeam_channel::bounded(1);
        {
            let mut lock = self.terminator.write().unwrap();
            *lock = Some(term_s);
        }

        thread::spawn(move || {
            let ticker = tick(delay);
            loop {
                select! {
                    recv(ticker) -> _ =>  {
                        let _ = regenerate_cache_from_replay(&conn, c.clone(), &replay_req_subject);
                    }
                    recv(term_r) -> _ => break,
                }
            }
        });

        Ok(())
    }

    fn health(&self) -> MessageHandlerResult {
        serialize(HealthCheckResponse {
            healthy: true,
            message: "".to_string(),
        })
    }

    fn add(&self, _actor: &str, req: AddArgs) -> MessageHandlerResult {
        let evt = CacheEvent::AtomicAdd(req.key, req.value);
        handle_state_event(&evt, self.cache.clone())?;
        publish_state_event(
            &self.id,
            &self.event_subject.read().unwrap(),
            &evt,
            self.nc.clone(),
        )?;
        serialize(AddResponse::default())
    }

    fn del(&self, _actor: &str, req: DelArgs) -> MessageHandlerResult {
        let evt = CacheEvent::KeyDelete(req.key);
        handle_state_event(&evt, self.cache.clone())?;
        publish_state_event(
            &self.id,
            &self.event_subject.read().unwrap(),
            &evt,
            self.nc.clone(),
        )?;
        serialize(DelResponse::default())
    }

    fn get(&self, _actor: &str, req: GetArgs) -> MessageHandlerResult {
        let resp = match self.cache.read().unwrap().get(&req.key) {
            Ok(v) if v.is_some() => GetResponse {
                value: v.unwrap(),
                exists: true,
            },
            Ok(_v) => GetResponse {
                value: "".to_string(),
                exists: false,
            },
            Err(e) => return Err(format!("Failed to retrieve value {}", e).into()),
        };
        serialize(resp)
    }

    fn list_clear(&self, _actor: &str, req: ClearArgs) -> MessageHandlerResult {
        let evt = CacheEvent::ListClear(req.key);
        handle_state_event(&evt, self.cache.clone())?;
        publish_state_event(
            &self.id,
            &self.event_subject.read().unwrap(),
            &evt,
            self.nc.clone(),
        )?;
        serialize(DelResponse::default())
    }

    fn list_range(&self, _actor: &str, req: RangeArgs) -> MessageHandlerResult {
        let resp = match self
            .cache
            .read()
            .unwrap()
            .list_range(&req.key, req.start, req.stop)
        {
            Ok(v) => v,
            Err(e) => return Err(format!("Failed to get list range: {}", e).into()),
        };
        serialize(ListRangeResponse { values: resp })
    }

    fn list_push(&self, _actor: &str, req: PushArgs) -> MessageHandlerResult {
        let evt = CacheEvent::ListPush(req.key, req.value);
        handle_state_event(&evt, self.cache.clone())?;
        publish_state_event(
            &self.id,
            &self.event_subject.read().unwrap(),
            &evt,
            self.nc.clone(),
        )?;
        serialize(ListResponse::default())
    }

    fn set(&self, _actor: &str, req: SetArgs) -> MessageHandlerResult {
        let evt = CacheEvent::ScalarSet(req.key, req.value);
        handle_state_event(&evt, self.cache.clone())?;
        publish_state_event(
            &self.id,
            &self.event_subject.read().unwrap(),
            &evt,
            self.nc.clone(),
        )?;
        serialize(SetResponse::default())
    }

    fn list_del_item(&self, _actor: &str, req: ListItemDeleteArgs) -> MessageHandlerResult {
        let evt = CacheEvent::ListRemoveItem(req.key, req.value);
        handle_state_event(&evt, self.cache.clone())?;
        publish_state_event(
            &self.id,
            &self.event_subject.read().unwrap(),
            &evt,
            self.nc.clone(),
        )?;
        serialize(ListResponse::default())
    }

    fn set_add(&self, _actor: &str, req: SetAddArgs) -> MessageHandlerResult {
        let evt = CacheEvent::SetAdd(req.key, req.value);
        handle_state_event(&evt, self.cache.clone())?;
        publish_state_event(
            &self.id,
            &self.event_subject.read().unwrap(),
            &evt,
            self.nc.clone(),
        )?;
        serialize(SetOperationResponse::default())
    }

    fn set_remove(&self, _actor: &str, req: SetRemoveArgs) -> MessageHandlerResult {
        let evt = CacheEvent::SetRemoveItem(req.key, req.value);
        handle_state_event(&evt, self.cache.clone())?;
        publish_state_event(
            &self.id,
            &self.event_subject.read().unwrap(),
            &evt,
            self.nc.clone(),
        )?;
        serialize(SetOperationResponse::default())
    }

    fn set_union(&self, _actor: &str, req: SetUnionArgs) -> MessageHandlerResult {
        let resp = match self.cache.read().unwrap().set_union(req.keys) {
            Ok(v) => v,
            Err(e) => return Err(format!("Failed to perform set untion: {}", e).into()),
        };
        serialize(SetQueryResponse { values: resp })
    }

    fn set_intersect(&self, _actor: &str, req: SetIntersectionArgs) -> MessageHandlerResult {
        let resp = match self.cache.read().unwrap().set_intersect(req.keys) {
            Ok(v) => v,
            Err(e) => return Err(format!("Failed to perform set intersect: {}", e).into()),
        };
        serialize(SetQueryResponse { values: resp })
    }

    fn set_query(&self, _actor: &str, req: SetQueryArgs) -> MessageHandlerResult {
        let resp = match self.cache.read().unwrap().set_query(&req.key) {
            Ok(v) => v,
            Err(e) => return Err(format!("Failed to query set members: {}", e).into()),
        };
        serialize(SetQueryResponse { values: resp })
    }

    fn exists(&self, _actor: &str, req: KeyExistsArgs) -> MessageHandlerResult {
        let resp = match self.cache.read().unwrap().exists(&req.key) {
            Ok(b) => b,
            Err(e) => return Err(format!("Unable to determine key existence: {}", e).into()),
        };
        serialize(GetResponse {
            value: "".to_string(),
            exists: resp,
        })
    }
}

fn nats_connection_from_values(
    values: HashMap<String, String>,
) -> Result<nats::Connection, Box<dyn std::error::Error + Sync + Send>> {
    let nats_url = values
        .get(NATS_URL_CONFIG_KEY)
        .map(|v| v.as_str())
        .unwrap_or(DEFAULT_NATS_URL);
    let mut opts = if let Some(seed) = values.get(CLIENT_SEED_CONFIG_KEY) {
        let jwt = values
            .get(CLIENT_JWT_CONFIG_KEY)
            .unwrap_or(&"".to_string())
            .to_string();
        let kp = KeyPair::from_seed(seed)?;
        nats::Options::with_jwt(
            move || Ok(jwt.to_string()),
            move |nonce| kp.sign(nonce).unwrap(),
        )
    } else {
        nats::Options::new()
    };
    opts = opts.with_name("wasmCloud KV Cache Provider");
    opts.connect(&nats_url)
        .map_err(|e| format!("NATS connection failure:{}", e).into())
}

fn process_replay_request(
    msg: nats::Message,
    cache: Arc<RwLock<CacheData>>,
) -> Result<(), std::io::Error> {
    let req: ReplayRequest =
        deserialize(&msg.data).map_err(|e| gen_std_io_error(&format!("{}", e)))?;
    let history = cache.read().unwrap().history();

    let ack = ack_from_request(req.hwm, history.len());
    msg.respond(&serialize(&ack).map_err(|e| gen_std_io_error(&format!("{}", e)))?)?;
    let start = history.len() - ack.events_to_expect as usize;
    let end = history.len();
    for evt in history.iter().take(end).skip(start) {
        msg.respond(&serialize(evt).map_err(|e| gen_std_io_error(&format!("{}", e)))?)?;
    }
    Ok(())
}

fn gen_std_io_error(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, msg)
}

fn regenerate_cache_from_replay(
    nc: &nats::Connection,
    cache: Arc<RwLock<CacheData>>,
    replay_req_subject: &str,
) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
    let sub = nc.request_multi(
        replay_req_subject,
        serialize(ReplayRequest {
            hwm: cache.read().unwrap().history().len() as u64,
        })?,
    )?;

    let first = sub.next_timeout(Duration::from_secs(2));
    if first.is_err() {
        return Ok(()); // No one is listening for replay requests. That's not necessarily fatal
    }
    let first = first.unwrap();

    let ack: ReplayRequestAck = deserialize(&first.data)?;
    for _i in 0..ack.events_to_expect {
        if let Ok(msg) = sub.next_timeout(Duration::from_secs(1)) {
            let evt: CacheEvent = deserialize(&msg.data)?;
            if let Err(e) = handle_state_event(&evt, cache.clone()) {
                error!(
                    "Failed processing cache state event: {}. Cache should be considered invalid.",
                    e
                );
            }
        } else {
            error!("Did not receive an expected state replication reply. Cache should now be considered invalid.");
        }
    }
    Ok(())
}

// Receive an inbound event, which just modifies our internal state
fn handle_state_event(
    evt: &CacheEvent,
    cache: Arc<RwLock<CacheData>>,
) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
    let new_state = {
        let state = cache.read().unwrap();
        Cache::apply_event(&state, &evt)
    }?;

    let mut lock = cache.write().unwrap();
    *lock = new_state;

    Ok(())
}

fn publish_state_event(
    origin: &str,
    subject: &str,
    evt: &CacheEvent,
    nc: Arc<RwLock<Option<nats::Connection>>>,
) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
    if let Some(ref conn) = *nc.read().unwrap() {
        let wrapper = CacheEventWrapper {
            origin_id: origin.to_string(),
            event: evt.clone(),
        };
        conn.publish(subject, serialize(wrapper)?)?;
    }
    Ok(())
}

impl CapabilityProvider for NatsReplicatedKVProvider {
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn Dispatcher>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        trace!("Dispatcher received.");
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => self.configure(deserialize(msg)?),
            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => self.remove_actor(deserialize(msg)?),
            OP_HEALTH_REQUEST if actor == SYSTEM_ACTOR => self.health(),
            keyvalue::OP_ADD => self.add(actor, deserialize(msg)?),
            keyvalue::OP_DEL => self.del(actor, deserialize(msg)?),
            keyvalue::OP_GET => self.get(actor, deserialize(msg)?),
            keyvalue::OP_CLEAR => self.list_clear(actor, deserialize(msg)?),
            keyvalue::OP_RANGE => self.list_range(actor, deserialize(msg)?),
            keyvalue::OP_PUSH => self.list_push(actor, deserialize(msg)?),
            keyvalue::OP_SET => self.set(actor, deserialize(msg)?),
            keyvalue::OP_LIST_DEL => self.list_del_item(actor, deserialize(msg)?),
            keyvalue::OP_SET_ADD => self.set_add(actor, deserialize(msg)?),
            keyvalue::OP_SET_REMOVE => self.set_remove(actor, deserialize(msg)?),
            keyvalue::OP_SET_UNION => self.set_union(actor, deserialize(msg)?),
            keyvalue::OP_SET_INTERSECT => self.set_intersect(actor, deserialize(msg)?),
            keyvalue::OP_SET_QUERY => self.set_query(actor, deserialize(msg)?),
            keyvalue::OP_KEY_EXISTS => self.exists(actor, deserialize(msg)?),
            _ => Err("bad dispatch".into()),
        }
    }

    fn stop(&self) {
        /*{
            let mut lock = self.terminator.write().unwrap();
            if let Some(t) = lock.as_mut() {
                let _ = t.send(true);
            }
        } */
        /*
        let mut lock = self.nc.write().unwrap();
        if let Some(nc) = lock.take() {
            nc.close();
        } */
    }
}

fn ack_from_request(req_hwm: u64, history_len: usize) -> ReplayRequestAck {
    let diff = history_len as i64 - req_hwm as i64;
    if diff <= 0 {
        ReplayRequestAck {
            events_to_expect: 0,
        }
    } else {
        ReplayRequestAck {
            events_to_expect: diff as u64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRequest {
    pub hwm: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplayRequestAck {
    pub events_to_expect: u64,
}

#[cfg(test)]
mod test {
    use crate::{ack_from_request, ReplayRequestAck};

    #[test]
    fn watermark_diffing() {
        // scenario:
        // requester sends hwm of 12 (local history length)
        // handler has hwm of 15 (local history length)
        // need 3 as the `events_to_expect` field.
        assert_eq!(
            ReplayRequestAck {
                events_to_expect: 3
            },
            ack_from_request(12, 15)
        );
    }
}

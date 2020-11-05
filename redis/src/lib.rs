pub(crate) mod generated;
mod kvredis;

#[macro_use]
extern crate wascc_codec as codec;

#[macro_use]
extern crate log;

use crate::generated::core::CapabilityConfiguration;
use crate::generated::keyvalue::*;
use codec::capabilities::{
    CapabilityDescriptor, CapabilityProvider, Dispatcher, NullDispatcher, OperationDirection,
    OP_GET_CAPABILITY_DESCRIPTOR,
};
use codec::core::{OP_BIND_ACTOR, OP_REMOVE_ACTOR};
use codec::{deserialize, serialize};
use redis::Connection;
use redis::RedisResult;
use redis::{self, Commands};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::sync::RwLock;

pub const OP_ADD: &str = "Add";
pub const OP_GET: &str = "Get";
pub const OP_SET: &str = "Set";
pub const OP_DEL: &str = "Del";
pub const OP_CLEAR: &str = "Clear";
pub const OP_RANGE: &str = "Range";
pub const OP_PUSH: &str = "Push";
pub const OP_LIST_DEL: &str = "ListItemDelete";

pub const OP_SET_ADD: &str = "SetAdd";
pub const OP_SET_REMOVE: &str = "SetRemove";
pub const OP_SET_UNION: &str = "SetUnion";
pub const OP_SET_INTERSECT: &str = "SetIntersection";
pub const OP_SET_QUERY: &str = "SetQuery";
pub const OP_KEY_EXISTS: &str = "KeyExists";

const CAPABILITY_ID: &str = "wascc:keyvalue";
const SYSTEM_ACTOR: &str = "system";
const REVISION: u32 = 2; // Increment for each crates publish
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(not(feature = "static_plugin"))]
capability_provider!(RedisKVProvider, RedisKVProvider::new);

/// Redis implementation of the `wascc:keyvalue` specification
#[derive(Clone)]
pub struct RedisKVProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    clients: Arc<RwLock<HashMap<String, redis::Client>>>,
}

impl Default for RedisKVProvider {
    fn default() -> Self {
        match env_logger::try_init() {
            Ok(_) => {}
            Err(_) => {}
        };

        RedisKVProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl RedisKVProvider {
    /// Creates a new Redis provider
    pub fn new() -> Self {
        RedisKVProvider::default()
    }

    fn actor_con(&self, actor: &str) -> RedisResult<Connection> {
        let lock = self.clients.read().unwrap();
        if let Some(client) = lock.get(actor) {
            client.get_connection()
        } else {
            Err(redis::RedisError::from((
                redis::ErrorKind::InvalidClientConfig,
                "No client for this actor. Did the host configure it?",
            )))
        }
    }

    fn configure(
        &self,
        config: CapabilityConfiguration,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let c = kvredis::initialize_client(config.clone())?;

        self.clients.write().unwrap().insert(config.module, c);
        Ok(vec![])
    }

    fn remove_actor(
        &self,
        config: CapabilityConfiguration,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        self.clients.write().unwrap().remove(&config.module);
        Ok(vec![])
    }

    fn add(&self, actor: &str, req: AddArgs) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        let res: i32 = con.incr(req.key, req.value)?;
        let resp = AddResponse { value: res };

        Ok(serialize(resp)?)
    }

    fn del(&self, actor: &str, req: DelArgs) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        con.del(&req.key)?;
        let resp = DelResponse { key: req.key };

        Ok(serialize(resp)?)
    }

    fn get(&self, actor: &str, req: GetArgs) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        if !con.exists(&req.key)? {
            Ok(serialize(GetResponse {
                value: String::from(""),
                exists: false,
            })?)
        } else {
            let v: redis::RedisResult<String> = con.get(&req.key);
            Ok(serialize(match v {
                Ok(s) => GetResponse {
                    value: s,
                    exists: true,
                },
                Err(e) => {
                    eprint!("GET for {} failed: {}", &req.key, e);
                    GetResponse {
                        value: "".to_string(),
                        exists: false,
                    }
                }
            })?)
        }
    }

    fn list_clear(
        &self,
        actor: &str,
        req: ClearArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        self.del(actor, DelArgs { key: req.key })
    }

    fn list_range(
        &self,
        actor: &str,
        req: RangeArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        let result: Vec<String> = con.lrange(req.key, req.start as _, req.stop as _)?;
        Ok(serialize(ListRangeResponse { values: result })?)
    }

    fn list_push(
        &self,
        actor: &str,
        req: PushArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        let result: i32 = con.lpush(req.key, req.value)?;
        Ok(serialize(ListResponse { new_count: result })?)
    }

    fn set(&self, actor: &str, req: SetArgs) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        con.set(req.key, &req.value)?;
        Ok(serialize(SetResponse {
            value: req.value.clone(),
        })?)
    }

    fn list_del_item(
        &self,
        actor: &str,
        req: ListItemDeleteArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        let result: i32 = con.lrem(req.key, 0, &req.value)?;
        Ok(serialize(ListResponse { new_count: result })?)
    }

    fn set_add(
        &self,
        actor: &str,
        req: SetAddArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        let result: i32 = con.sadd(req.key, &req.value)?;
        Ok(serialize(SetOperationResponse { new_count: result })?)
    }

    fn set_remove(
        &self,
        actor: &str,
        req: SetRemoveArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        let result: i32 = con.srem(req.key, &req.value)?;
        Ok(serialize(SetOperationResponse { new_count: result })?)
    }

    fn set_union(
        &self,
        actor: &str,
        req: SetUnionArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        let result: Vec<String> = con.sunion(req.keys)?;
        Ok(serialize(SetQueryResponse { values: result })?)
    }

    fn set_intersect(
        &self,
        actor: &str,
        req: SetIntersectionArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        let result: Vec<String> = con.sinter(req.keys)?;
        Ok(serialize(SetQueryResponse { values: result })?)
    }

    fn set_query(
        &self,
        actor: &str,
        req: SetQueryArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        let result: Vec<String> = con.smembers(req.key)?;
        Ok(serialize(SetQueryResponse { values: result })?)
    }

    fn exists(
        &self,
        actor: &str,
        req: KeyExistsArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut con = self.actor_con(actor)?;
        let result: bool = con.exists(req.key)?;
        Ok(serialize(GetResponse {
            value: "".to_string(),
            exists: result,
        })?)
    }

    fn get_descriptor(&self) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        use OperationDirection::ToProvider;
        Ok(serialize(
            CapabilityDescriptor::builder()
                .id(CAPABILITY_ID)
                .name("waSCC Default Key-Value Provider (Redis)")
                .long_description("A key-value store capability provider built on Redis")
                .version(VERSION)
                .revision(REVISION)
                .with_operation(OP_ADD, ToProvider, "Performs an atomic addition operation")
                .with_operation(OP_DEL, ToProvider, "Deletes a key from the store")
                .with_operation(OP_GET, ToProvider, "Gets the raw value for a key")
                .with_operation(OP_CLEAR, ToProvider, "Clears a list")
                .with_operation(
                    OP_RANGE,
                    ToProvider,
                    "Selects items from a list within a range",
                )
                .with_operation(OP_PUSH, ToProvider, "Pushes a new item onto a list")
                .with_operation(OP_SET, ToProvider, "Sets the value of a key")
                .with_operation(OP_LIST_DEL, ToProvider, "Deletes an item from a list")
                .with_operation(OP_SET_ADD, ToProvider, "Adds an item to a set")
                .with_operation(OP_SET_REMOVE, ToProvider, "Remove an item from a set")
                .with_operation(
                    OP_SET_UNION,
                    ToProvider,
                    "Returns the union of multiple sets",
                )
                .with_operation(
                    OP_SET_INTERSECT,
                    ToProvider,
                    "Returns the intersection of multiple sets",
                )
                .with_operation(OP_SET_QUERY, ToProvider, "Queries a set")
                .with_operation(
                    OP_KEY_EXISTS,
                    ToProvider,
                    "Returns a boolean indicating if a key exists",
                )
                .build(),
        )?)
    }
}

impl CapabilityProvider for RedisKVProvider {
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn Dispatcher>,
    ) -> Result<(), Box<dyn Error + Sync + Send>> {
        trace!("Dispatcher received.");

        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    fn stop(&self) {
        let mut lock = self.clients.write().unwrap();
        lock.clear();
        *self.dispatcher.write().unwrap() = Box::new(NullDispatcher::new());
    }

    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        trace!(
            "Received host call from {}, operation - {} ({} bytes)",
            actor,
            op,
            msg.len()
        );

        match op {
            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => {
                self.configure(deserialize::<CapabilityConfiguration>(msg).unwrap())
            }
            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => {
                self.remove_actor(deserialize::<CapabilityConfiguration>(msg).unwrap())
            }
            OP_GET_CAPABILITY_DESCRIPTOR if actor == SYSTEM_ACTOR => self.get_descriptor(),
            OP_ADD => self.add(actor, deserialize(msg).unwrap()),
            OP_DEL => self.del(actor, deserialize(msg).unwrap()),
            OP_GET => self.get(actor, deserialize(msg).unwrap()),
            OP_CLEAR => self.list_clear(actor, deserialize(msg).unwrap()),
            OP_RANGE => self.list_range(actor, deserialize(msg).unwrap()),
            OP_PUSH => self.list_push(actor, deserialize(msg).unwrap()),
            OP_SET => self.set(actor, deserialize(msg).unwrap()),
            OP_LIST_DEL => self.list_del_item(actor, deserialize(msg).unwrap()),
            OP_SET_ADD => self.set_add(actor, deserialize(msg).unwrap()),
            OP_SET_REMOVE => self.set_remove(actor, deserialize(msg).unwrap()),
            OP_SET_UNION => self.set_union(actor, deserialize(msg).unwrap()),
            OP_SET_INTERSECT => self.set_intersect(actor, deserialize(msg).unwrap()),
            OP_SET_QUERY => self.set_query(actor, deserialize(msg).unwrap()),
            OP_KEY_EXISTS => self.exists(actor, deserialize(msg).unwrap()),
            _ => Err("bad dispatch".into()),
        }
    }
}

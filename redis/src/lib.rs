mod kvredis;

#[macro_use]
extern crate wasmcloud_provider_core as codec;
use actorcore::{deserialize, serialize, CapabilityConfiguration, HealthCheckResponse};
use actorkeyvalue::*;
use codec::{
    capabilities::{CapabilityProvider, Dispatcher, NullDispatcher},
    core::{OP_BIND_ACTOR, OP_HEALTH_REQUEST, OP_REMOVE_ACTOR},
};
use log::trace;
use redis::Connection;
use redis::RedisResult;
use redis::{self, Commands};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::sync::RwLock;
use wasmcloud_actor_core as actorcore;
use wasmcloud_actor_keyvalue as actorkeyvalue;

#[allow(unused)]
const CAPABILITY_ID: &str = "wasmcloud:keyvalue";
const SYSTEM_ACTOR: &str = "system";

#[cfg(not(feature = "static_plugin"))]
capability_provider!(RedisKVProvider, RedisKVProvider::new);

/// Redis implementation of the `wasmcloud:keyvalue` specification
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
        if self.clients.read().unwrap().contains_key(&config.module) {
            return Ok(vec![]);
        }
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
        // Nothing to do here
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
            OP_HEALTH_REQUEST if actor == SYSTEM_ACTOR => Ok(serialize(HealthCheckResponse {
                healthy: true,
                message: "".to_string(),
            })
            .unwrap()),
            _ => Err("bad dispatch".into()),
        }
    }
}

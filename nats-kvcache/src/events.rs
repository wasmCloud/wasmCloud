use std::error::Error;

use eventsourcing::prelude::*;
use serde::{Deserialize, Serialize};

use crate::kvcache::KeyValueStore;

pub(crate) const DOMAIN_VERSION: &str = "1.0";

#[derive(Serialize, Deserialize, Debug, Clone, Event)]
#[event_type_version(DOMAIN_VERSION)]
#[event_source("events://github.com/wasmCloud/capability-providers/rust/nats-keyvalue")]
/// Indicates an event that has occurred on a distributed cache. In all of the variants of this event, the first parameter is always the key
pub enum CacheEvent {
    AtomicAdd(String, i32),
    KeyDelete(String),
    ScalarSet(String, String),
    ListClear(String),
    ListPush(String, String),
    ListRemoveItem(String, String),
    SetAdd(String, String),
    SetRemoveItem(String, String),
}

#[doc(hidden)]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CacheEventWrapper {
    pub origin_id: String,
    pub event: CacheEvent,
}

pub(crate) enum CacheCommand {}

#[derive(Clone, Debug)]
pub(crate) struct CacheData {
    history: Vec<CacheEvent>,
    cache: KeyValueStore,
}

impl CacheData {
    pub fn new(cache: KeyValueStore) -> Self {
        CacheData {
            history: vec![],
            cache,
        }
    }

    pub fn history(&self) -> Vec<CacheEvent> {
        self.history.clone()
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, Box<dyn Error>> {
        if self.cache.exists(key)? {
            Ok(Some(self.cache.get(key)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_range(
        &self,
        key: &str,
        start: i32,
        stop: i32,
    ) -> Result<Vec<String>, Box<dyn Error>> {
        self.cache.lrange(key, start, stop)
    }

    pub fn exists(&self, key: &str) -> Result<bool, Box<dyn Error>> {
        self.cache.exists(key)
    }

    pub fn set_union(&self, keys: Vec<String>) -> Result<Vec<String>, Box<dyn Error>> {
        self.cache.sunion(keys)
    }

    pub fn set_intersect(&self, keys: Vec<String>) -> Result<Vec<String>, Box<dyn Error>> {
        self.cache.sinter(keys)
    }

    pub fn set_query(&self, key: &str) -> Result<Vec<String>, Box<dyn Error>> {
        self.cache.smembers(key.to_string())
    }

    pub fn atomic_add(
        &mut self,
        key: &str,
        value: &i32,
    ) -> ::std::result::Result<(), Box<dyn Error>> {
        self.cache.incr(key, *value).map(|_| ())
    }

    pub fn key_delete(&mut self, key: &str) -> Result<(), Box<dyn Error>> {
        self.cache.del(key)
    }

    pub fn set_scalar(&mut self, key: &str, value: &str) -> Result<(), Box<dyn Error>> {
        self.cache.set(key, value.to_string())
    }

    pub fn list_clear(&mut self, key: &str) -> Result<(), Box<dyn Error>> {
        self.cache.del(key)
    }

    pub fn list_push(&mut self, key: &str, value: &str) -> Result<(), Box<dyn Error>> {
        self.cache.lpush(key, value.to_string()).map(|_| ())
    }

    pub fn list_remove(&mut self, key: &str, value: &str) -> Result<(), Box<dyn Error>> {
        self.cache.lrem(key, value.to_string()).map(|_| ())
    }

    pub fn set_add(&mut self, key: &str, value: &str) -> Result<(), Box<dyn Error>> {
        self.cache.sadd(key, value.to_string()).map(|_| ())
    }

    pub fn set_remove(&mut self, key: &str, value: &str) -> Result<(), Box<dyn Error>> {
        self.cache.srem(key, value.to_string()).map(|_| ())
    }

    pub fn log_event(&mut self, evt: CacheEvent) {
        self.history.push(evt);
    }
}

impl AggregateState for CacheData {
    fn generation(&self) -> u64 {
        self.history.len() as u64
    }
}

pub(crate) struct Cache;
impl Aggregate for Cache {
    type Event = CacheEvent;
    type Command = CacheCommand;
    type State = CacheData;

    fn apply_event(state: &Self::State, evt: &Self::Event) -> eventsourcing::Result<Self::State> {
        let mut state = state.clone();
        match evt {
            CacheEvent::AtomicAdd(key, value) => state.atomic_add(key, value),
            CacheEvent::KeyDelete(key) => state.key_delete(key),
            CacheEvent::ScalarSet(key, value) => state.set_scalar(key, value),
            CacheEvent::ListClear(key) => state.list_clear(key),
            CacheEvent::ListPush(key, value) => state.list_push(key, value),
            CacheEvent::ListRemoveItem(key, value) => state.list_remove(key, value),
            CacheEvent::SetAdd(key, value) => state.set_add(key, value),
            CacheEvent::SetRemoveItem(key, value) => state.set_remove(key, value),
        }
        .map_err(|e| eventsourcing::Error {
            kind: eventsourcing::Kind::StoreFailure(format!("{}", e)),
        })?;
        state.log_event(evt.clone());
        Ok(state)
    }

    fn handle_command(
        _state: &Self::State,
        _cmd: &Self::Command,
    ) -> eventsourcing::Result<Vec<Self::Event>> {
        todo!()
    }
}

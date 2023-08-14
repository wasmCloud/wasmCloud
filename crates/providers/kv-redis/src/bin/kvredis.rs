//! Redis implementation for wasmcloud:keyvalue.
//!
//! This implementation is multi-threaded and operations between different actors
//! use different connections and can run in parallel.
//! A single connection is shared by all instances of the same actor id (public key),
//! so there may be some brief lock contention if several instances of the same actor
//! are simultaneously attempting to communicate with redis. See documentation
//! on the [exec](#exec) function for more information.
//!
//!
use std::collections::HashMap;
use std::ops::DerefMut;
use std::sync::Arc;

use redis::aio::ConnectionManager;
use redis::FromRedisValue;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{info, instrument, warn};
use wasmcloud_provider_kvredis::wasmcloud_interface_keyvalue::{
    GetResponse, IncrementRequest, KeyValue, ListAddRequest, ListDelRequest, ListRangeRequest,
    SetAddRequest, SetDelRequest, SetRequest, StringList,
};
use wasmcloud_provider_sdk::{
    core::LinkDefinition, error::ProviderInvocationError, load_host_data,
    provider_main::start_provider, Context, ProviderHandler,
};

const REDIS_URL_KEY: &str = "URL";
const DEFAULT_CONNECT_URL: &str = "redis://127.0.0.1:6379/";

#[derive(Deserialize)]
struct KvRedisConfig {
    /// Default URL to connect when actor doesn't provide one on a link
    #[serde(alias = "URL", alias = "Url")]
    url: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let hd = load_host_data()?;

    let default_connect_url = if let Some(raw_config) = hd.config_json.as_ref() {
        match serde_json::from_str(raw_config) {
            Ok(KvRedisConfig { url }) => {
                info!(url, "Using Redis URL from config");
                url
            }
            Err(err) => {
                warn!(
                    DEFAULT_CONNECT_URL,
                    "Failed to parse `config_json`: {err}\nUsing default configuration"
                );
                DEFAULT_CONNECT_URL.to_string()
            }
        }
    } else {
        info!(DEFAULT_CONNECT_URL, "Using default Redis URL");
        DEFAULT_CONNECT_URL.to_string()
    };

    start_provider(
        KvRedisProvider::new(&default_connect_url),
        Some("KeyValue Redis Provider".to_string()),
    )?;

    eprintln!("KVRedis provider exiting");
    Ok(())
}

/// Redis keyValue provider implementation.
#[derive(Default, Clone)]
struct KvRedisProvider {
    // store redis connections per actor
    actors: Arc<RwLock<HashMap<String, RwLock<ConnectionManager>>>>,
    // Default connection URL for actors without a `URL` link value
    default_connect_url: String,
}

impl KvRedisProvider {
    fn new(default_connect_url: &str) -> Self {
        KvRedisProvider {
            default_connect_url: default_connect_url.to_string(),
            ..Default::default()
        }
    }
}

/// Handle provider control commands
/// put_link (new actor link command), del_link (remove link command), and shutdown
#[async_trait::async_trait]
impl ProviderHandler for KvRedisProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip(self, ld), fields(actor_id = %ld.actor_id))]
    async fn put_link(&self, ld: &LinkDefinition) -> bool {
        let redis_url = get_redis_url(&ld.values, &self.default_connect_url);

        match redis::Client::open(redis_url.clone()) {
            Ok(client) => match client.get_tokio_connection_manager().await {
                Ok(conn_manager) => {
                    info!(redis_url, "established link");
                    let mut update_map = self.actors.write().await;
                    update_map.insert(ld.actor_id.to_string(), RwLock::new(conn_manager));
                }
                Err(err) => {
                    warn!(
                        redis_url,
                        ?err,
                    "Could not create Redis connection manager for actor {}, keyvalue operations will fail",
                    ld.actor_id
                );
                    return false;
                }
            },
            Err(err) => {
                warn!(
                    ?err,
                    "Could not create Redis client for actor {}, keyvalue operations will fail",
                    ld.actor_id
                );
                return false;
            }
        }

        true
    }

    /// Handle notification that a link is dropped - close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, actor_id: &str) {
        let mut aw = self.actors.write().await;
        if let Some(conn) = aw.remove(actor_id) {
            info!("redis closing connection for actor {}", actor_id);
            drop(conn)
        }
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) {
        let mut aw = self.actors.write().await;
        // empty the actor link data and stop all servers
        for (_, conn) in aw.drain() {
            drop(conn)
        }
    }
}

// There are two api styles you can use for invoking redis. You can build any raw command
// as a string command and a sequence of args:
// ```
//     let mut cmd = redis::cmd("SREM");
//     let value: u32 = self.exec(&ctx, &mut cmd.arg(&arg.set_name).arg(&arg.value)).await?;
// ```
// or you can call a method on Cmd, as in
// ```
//     let mut cmd = redis::Cmd::srem(&arg.set_name, &arg.value);
//     let value: u32 = self.exec(&ctx, &mut cmd).await?;
//```
// The latter api style has better rust compile-time type checking for args.
// The rust docs for cmd and Cmd don't document arg types or return types.
// For that, you need to look at https://redis.io/commands#

/// Handle KeyValue methods that interact with redis
#[async_trait::async_trait]
impl KeyValue for KvRedisProvider {
    /// Increments a numeric value, returning the new value
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.key))]
    async fn increment(&self, ctx: Context, arg: IncrementRequest) -> Result<i32, String> {
        let mut cmd = redis::Cmd::incr(&arg.key, arg.value);
        let val: i32 = self.exec(&ctx, &mut cmd).await?;
        Ok(val)
    }

    /// Returns true if the store contains the key
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.to_string()))]
    async fn contains(&self, ctx: Context, arg: String) -> Result<bool, String> {
        let mut cmd = redis::Cmd::exists(arg.to_string());
        let val: bool = self.exec(&ctx, &mut cmd).await?;
        Ok(val)
    }

    /// Deletes a key, returning true if the key was deleted
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.to_string()))]
    async fn del(&self, ctx: Context, arg: String) -> Result<bool, String> {
        let mut cmd = redis::Cmd::del(arg.to_string());
        let val: i32 = self.exec(&ctx, &mut cmd).await?;
        Ok(val > 0)
    }

    /// Gets a value for a specified key. If the key exists,
    /// the return structure contains exists: true and the value,
    /// otherwise the return structure contains exists == false.
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.to_string()))]
    async fn get(&self, ctx: Context, arg: String) -> Result<GetResponse, String> {
        let mut cmd = redis::Cmd::get(arg.to_string());
        let val: Option<String> = self.exec(&ctx, &mut cmd).await?;
        let resp = match val {
            Some(s) => GetResponse {
                exists: true,
                value: s,
            },
            None => GetResponse {
                exists: false,
                ..Default::default()
            },
        };
        Ok(resp)
    }

    /// Append a value onto the end of a list. Returns the new list size
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.list_name))]
    async fn list_add(&self, ctx: Context, arg: ListAddRequest) -> Result<u32, String> {
        let mut cmd = redis::Cmd::rpush(&arg.list_name, &arg.value);
        let val: u32 = self.exec(&ctx, &mut cmd).await?;
        Ok(val)
    }

    /// Deletes a list and its contents
    /// input: list name
    /// returns: true if the list existed and was deleted
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.to_string()))]
    async fn list_clear(&self, ctx: Context, arg: String) -> Result<bool, String> {
        self.del(ctx, arg).await
    }

    /// Deletes an item from a list. Returns true if the item was removed.
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.list_name))]
    async fn list_del(&self, ctx: Context, arg: ListDelRequest) -> Result<bool, String> {
        let mut cmd = redis::Cmd::lrem(&arg.list_name, 1, &arg.value);
        let val: u32 = self.exec(&ctx, &mut cmd).await?;
        Ok(val > 0)
    }

    /// Retrieves a range of values from a list using 0-based indices.
    /// Start and end values are inclusive, for example, (0,10) returns
    /// 11 items if the list contains at least 11 items. If the stop value
    /// is beyond the end of the list, it is treated as the end of the list.
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.list_name))]
    async fn list_range(&self, ctx: Context, arg: ListRangeRequest) -> Result<StringList, String> {
        let mut cmd = redis::Cmd::lrange(&arg.list_name, arg.start as isize, arg.stop as isize);
        let val: StringList = self.exec(&ctx, &mut cmd).await?;
        Ok(val)
    }

    /// Sets the value of a key.
    /// expires is an optional number of seconds before the value should be automatically deleted,
    /// or 0 for no expiration.
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.key))]
    async fn set(&self, ctx: Context, arg: SetRequest) -> Result<(), String> {
        let mut cmd = match arg.expires {
            0 => redis::Cmd::set(&arg.key, &arg.value),
            _ => redis::Cmd::set_ex(&arg.key, &arg.value, arg.expires as usize),
        };
        let _value: Option<String> = self.exec(&ctx, &mut cmd).await?;
        Ok(())
    }

    /// Add an item into a set. Returns number of items added
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.set_name))]
    async fn set_add(&self, ctx: Context, arg: SetAddRequest) -> Result<u32, String> {
        let mut cmd = redis::Cmd::sadd(&arg.set_name, &arg.value);
        let value: u32 = self.exec(&ctx, &mut cmd).await?;
        Ok(value)
    }

    /// Remove a item from the set. Returns
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.set_name))]
    async fn set_del(&self, ctx: Context, arg: SetDelRequest) -> Result<u32, String> {
        let mut cmd = redis::Cmd::srem(&arg.set_name, &arg.value);
        let value: u32 = self.exec(&ctx, &mut cmd).await?;
        Ok(value)
    }

    /// Deletes a set and its contents
    /// input: set name
    /// returns: true if the set existed and was deleted
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.to_string()))]
    async fn set_clear(&self, ctx: Context, arg: String) -> Result<bool, String> {
        self.del(ctx, arg).await
    }

    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, keys = ?arg))]
    async fn set_intersection(&self, ctx: Context, arg: StringList) -> Result<StringList, String> {
        let mut cmd = redis::Cmd::sinter(arg);
        let value: Vec<String> = self.exec(&ctx, &mut cmd).await?;
        Ok(value)
    }

    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.to_string()))]
    async fn set_query(&self, ctx: Context, arg: String) -> Result<StringList, String> {
        let mut cmd = redis::Cmd::smembers(arg.to_string());
        let values: Vec<String> = self.exec(&ctx, &mut cmd).await?;
        Ok(values)
    }

    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, keys = ?arg))]
    async fn set_union(&self, ctx: Context, arg: StringList) -> Result<StringList, String> {
        let mut cmd = redis::Cmd::sunion(arg);
        let values: Vec<String> = self.exec(&ctx, &mut cmd).await?;
        Ok(values)
    }
}

impl KvRedisProvider {
    /// Helper function to execute redis async command while holding onto a mutable connection.
    ///
    /// This provider is multi-threaded, and requests from different actors use
    /// different connections, and requests can run in parallel.
    ///
    /// There is a single connection per actor public key, and the write lock on the connection
    /// effectively serializes redis operations for all instances of the same actor.
    /// The lock is held only for the duration of a redis command from this provider
    /// and waiting for its response. The lock duration does not overlap with
    /// message passing between actors and this provider, including serialization
    /// of requests and deserialization of responses, which are fully parallelizable.
    ///
    /// There is a read lock held on the actors hashtable, which does not interfere
    /// with redis operations, but any control commands for new actor links
    /// or removal of actor links may need to wait for in-progress operations to complete.
    /// That should be rare, because most links are passed to the provider at startup.
    async fn exec<T: FromRedisValue>(
        &self,
        ctx: &Context,
        cmd: &mut redis::Cmd,
    ) -> Result<T, String> {
        let actor_id = ctx
            .actor
            .as_ref()
            .ok_or_else(|| "no actor in request".to_string())?;
        // get read lock on actor-connections hashmap
        let rd = self.actors.read().await;
        let rc = rd
            .get(actor_id)
            .ok_or_else(||format!("No Redis connection found for {}. Please ensure the URL supplied in the link definition is a valid Redis URL", actor_id))?;
        // get write lock on this actor's connection
        let mut con = rc.write().await;
        cmd.query_async(con.deref_mut())
            .await
            .map_err(|e| e.to_string())
    }
}

fn get_redis_url(link_values: &[(String, String)], default_connect_url: &str) -> String {
    link_values
        .iter()
        .find(|(key, _value)| key.eq_ignore_ascii_case(REDIS_URL_KEY))
        .map(|(_key, url)| url.to_owned())
        .unwrap_or_else(|| default_connect_url.to_owned())
}

#[async_trait::async_trait]
impl wasmcloud_provider_sdk::MessageDispatch for KvRedisProvider {
    async fn dispatch<'a>(
        &'a self,
        ctx: Context,
        method: String,
        body: std::borrow::Cow<'a, [u8]>,
    ) -> Result<Vec<u8>, ProviderInvocationError> {
        match method.as_str() {
            "KeyValue.Increment" => {
                let input: IncrementRequest = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.increment(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.Contains" => {
                let input: String = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.contains(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.Del" => {
                let input: String = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.del(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.Get" => {
                let input: String = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.get(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.ListAdd" => {
                let input: ListAddRequest = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.list_add(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.ListClear" => {
                let input: String = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.list_clear(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.ListRange" => {
                let input: ListRangeRequest = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.list_range(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.Set" => {
                let input: SetRequest = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.set(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.SetAdd" => {
                let input: SetAddRequest = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.set_add(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.SetDel" => {
                let input: SetDelRequest = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.set_del(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.SetIntersection" => {
                let input: StringList = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.set_intersection(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.SetQuery" => {
                let input: String = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.set_query(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.SetUnion" => {
                let input: StringList = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.set_union(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            "KeyValue.SetClear" => {
                let input: String = ::wasmcloud_provider_sdk::deserialize(&body)?;
                let result = self.set_clear(ctx, input).await.map_err(|e| {
                    ::wasmcloud_provider_sdk::error::ProviderInvocationError::Provider(
                        e.to_string(),
                    )
                })?;
                Ok(::wasmcloud_provider_sdk::serialize(&result)?)
            }
            _ => Err(
                ::wasmcloud_provider_sdk::error::InvocationError::Malformed(format!(
                    "Invalid method name {method}",
                ))
                .into(),
            ),
        }
    }
}

impl wasmcloud_provider_sdk::Provider for KvRedisProvider {}

#[cfg(test)]
mod test {
    use super::{get_redis_url, KvRedisConfig};

    const PROPER_URL: &str = "redis://127.0.0.1:6379";

    #[test]
    fn can_deserialize_config_case_insensitive() {
        let lowercase_config = format!("{{\"url\": \"{}\"}}", PROPER_URL);
        let uppercase_config = format!("{{\"URL\": \"{}\"}}", PROPER_URL);
        let initial_caps_config = format!("{{\"Url\": \"{}\"}}", PROPER_URL);

        assert_eq!(
            PROPER_URL,
            serde_json::from_str::<KvRedisConfig>(&lowercase_config)
                .unwrap()
                .url
        );
        assert_eq!(
            PROPER_URL,
            serde_json::from_str::<KvRedisConfig>(&uppercase_config)
                .unwrap()
                .url
        );
        assert_eq!(
            PROPER_URL,
            serde_json::from_str::<KvRedisConfig>(&initial_caps_config)
                .unwrap()
                .url
        );
    }

    #[test]
    fn can_accept_case_insensitive_url_parameters() {
        assert_eq!(
            get_redis_url(&[("url".to_string(), PROPER_URL.to_string())], ""),
            PROPER_URL
        );

        assert_eq!(
            get_redis_url(&[("URL".to_string(), PROPER_URL.to_string())], ""),
            PROPER_URL
        );

        assert_eq!(
            get_redis_url(&[("uRl".to_string(), PROPER_URL.to_string())], ""),
            PROPER_URL
        );

        assert_eq!(
            get_redis_url(&[("UrL".to_string(), PROPER_URL.to_string())], ""),
            PROPER_URL
        );
    }
}

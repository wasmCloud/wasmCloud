//! Redis implementation for wrpc:keyvalue.
//!
//! This implementation is multi-threaded and operations between different actors
//! use different connections and can run in parallel.
//! A single connection is shared by all instances of the same actor id (public key),
//! so there may be some brief lock contention if several instances of the same actor
//! are simultaneously attempting to communicate with redis. See documentation
//! on the [exec](#exec) function for more information.

use anyhow::Context as _;
use wasmcloud_core::HostData;
use wasmcloud_provider_keyvalue_redis::{retrieve_default_url, DefaultConnection, KvRedisProvider};
use wasmcloud_provider_sdk::interfaces::keyvalue::run;
use wasmcloud_provider_sdk::load_host_data;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let HostData { config, .. } = load_host_data().context("failed to load host data")?;
    let client = redis::Client::open(retrieve_default_url(config))
        .context("failed to construct default Redis client")?;
    let default_connection = if let Ok(conn) = client.get_connection_manager().await {
        DefaultConnection::Conn(conn)
    } else {
        DefaultConnection::Client(client)
    };
    run(
        KvRedisProvider::new(default_connection),
        "kv-redis-provider",
    )
    .await
    .context("failed to run provider")?;
    eprintln!("KVRedis provider exiting");
    Ok(())
}

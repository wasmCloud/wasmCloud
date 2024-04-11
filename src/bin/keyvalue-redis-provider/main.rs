//! Redis implementation for wrpc:keyvalue.
//!
//! This implementation is multi-threaded and operations between different actors
//! use different connections and can run in parallel.
//! A single connection is shared by all instances of the same actor id (public key),
//! so there may be some brief lock contention if several instances of the same actor
//! are simultaneously attempting to communicate with redis. See documentation
//! on the [exec](#exec) function for more information.

use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_keyvalue_redis::run()
        .await
        .context("failed to run provider")?;
    eprintln!("KVRedis provider exiting");
    Ok(())
}

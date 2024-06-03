//! NATS implementation of the wasmcloud "wrpc:keyvalue" capability contract

use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_keyvalue_nats::run()
        .await
        .context("failed to run provider")?;
    eprintln!("NATS Kv provider exiting");
    Ok(())
}

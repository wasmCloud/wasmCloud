//! Hashicorp Vault implementation of the wasmcloud `KeyValue` capability contract "wrpc:keyvalue"

use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_keyvalue_vault::run()
        .await
        .context("failed to run provider")?;
    eprintln!("KvVault provider exiting");
    Ok(())
}

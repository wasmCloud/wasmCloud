//! Hashicorp Vault implementation of the wasmcloud KeyValue capability contract "wrpc:keyvalue"

use anyhow::Context as _;
use wasmcloud_provider_keyvalue_vault::KvVaultProvider;
use wasmcloud_provider_sdk::interfaces::keyvalue::run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run(KvVaultProvider::default(), "kv-vault-provider")
        .await
        .context("failed to run provider")?;
    eprintln!("KvVault provider exiting");
    Ok(())
}

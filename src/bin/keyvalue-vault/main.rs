//! Hashicorp Vault implementation of the wasmcloud KeyValue capability contract "wrpc:keyvalue"

use anyhow::Context as _;
use wasmcloud_provider_keyvalue_vault::KvVaultProvider;
use wasmcloud_provider_sdk::interfaces::keyvalue::serve_keyvalue;
use wasmcloud_provider_sdk::run_provider_handler;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = KvVaultProvider::default();
    let fut = run_provider_handler(provider.clone(), "kv-vault-provider")
        .await
        .context("failed to run provider")?;
    serve_keyvalue(provider, fut).await?;
    eprintln!("KvVault provider exiting");
    Ok(())
}

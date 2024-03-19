//! Hashicorp Vault implementation of the wasmcloud KeyValue capability contract "wrpc:keyvalue"

use wasmcloud_provider_keyvalue_vault::KvVaultProvider;
use wasmcloud_provider_sdk::start_provider;

fn main() -> anyhow::Result<()> {
    start_provider(KvVaultProvider::default(), "kv-vault-provider")?;
    eprintln!("KvVault provider exiting");
    Ok(())
}

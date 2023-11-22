//! Hashicorp Vault implementation of the wasmcloud KeyValue capability contract "wasmcloud:keyvalue"
//!

use wasmcloud_provider_kv_vault::KvVaultProvider;
use wasmcloud_provider_sdk::provider_main::start_provider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    start_provider(
        KvVaultProvider::default(),
        Some("kv-vault-provider".to_string()),
    )?;

    eprintln!("KvVault provider exiting");
    Ok(())
}

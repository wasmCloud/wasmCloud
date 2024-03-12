//! Hashicorp Vault implementation of the wasmcloud KeyValue capability contract "wasmcloud:keyvalue"
//!

use wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk;

use wasmcloud_provider_kv_vault::KvVaultProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    wasmcloud_provider_sdk::start_provider(KvVaultProvider::default(), "kv-vault-provider")?;

    eprintln!("KvVault provider exiting");
    Ok(())
}

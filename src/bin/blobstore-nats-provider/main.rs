//! NATS implementation of the wasmcloud "wrpc:blobstore" capability contract

#![allow(warnings)]
use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_blobstore_nats::NatsBlobstoreProvider::run()
        .await
        .context("failed to run NATS blobstore provider")?;
    eprintln!("Blobstore NATS Provider exiting");
    Ok(())
}

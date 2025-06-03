//! This provider is a template that's meant to inform developers how to build a custom capability provider.
//!
//! The implementation in `./provider.rs` uses the `wasmcloud-provider-sdk` to provide a scaffold
//! for building a capability provider with a custom interface. Take note of the documentation
//! comments in the code to understand how to build a capability provider.

mod config;
mod provider;

use provider::CustomTemplateProvider;

/// Capability providers are native executables, so the entrypoint is the same as any other Rust
/// binary, `main()`. Typically the `main` function is kept simple and the provider logic is
/// implemented in a separate module. Head to the `provider.rs` file to see the implementation of
/// the `BlankSlateProvider`.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    CustomTemplateProvider::run().await?;
    eprintln!("Custom template provider exiting");
    Ok(())
}

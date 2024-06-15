use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_outgoing_email::run()
        .await
        .context("failed to run provider")?;
    eprintln!("Outgoing Email provider exiting");
    Ok(())
}

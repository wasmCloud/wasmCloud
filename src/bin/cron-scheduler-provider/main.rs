use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_cron_scheduler::run()
        .await
        .context("failed to run provider")?;
    eprintln!("Kafka messaging provider exiting");
    Ok(())
}

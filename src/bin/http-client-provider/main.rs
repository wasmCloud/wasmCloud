use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_http_client::run()
        .await
        .context("failed to run provider")?;
    eprintln!("HttpClient provider exiting");
    Ok(())
}

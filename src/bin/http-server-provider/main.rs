use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_http_server::run()
        .await
        .context("failed to run provider")?;
    eprintln!("HttpServer provider exiting");
    Ok(())
}

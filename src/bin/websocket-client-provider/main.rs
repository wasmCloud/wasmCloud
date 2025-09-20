use wasmcloud_provider_websocket_client::run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run().await
} 
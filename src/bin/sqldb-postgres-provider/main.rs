use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_sqldb_postgres::run()
        .await
        .context("failed to run provider")?;
    eprintln!("SQLDB Postgres Provider exiting");
    Ok(())
}

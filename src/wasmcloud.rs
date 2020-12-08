use wasmcloud_host::{HostBuilder, Result};

#[macro_use]
extern crate log;

#[actix_rt::main]
async fn main() -> Result<()> {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().filter_or(
        env_logger::DEFAULT_FILTER_ENV,
        "wasmcloud=info,wasmcloud_host=info",
    ))
    .format_module_path(false)
    .try_init();

    // TODO get this information from env vars/structopt
    let nc_rpc = nats::asynk::connect("0.0.0.0:4222").await?;
    let nc_control = nats::asynk::connect("0.0.0.0:4222").await?;

    let host = HostBuilder::new()
        .with_rpc_client(nc_rpc)
        .with_control_client(nc_control)
        .enable_live_updates()
        .build();
    match host.start().await {
        Ok(_) => {
            actix_rt::signal::ctrl_c().await.unwrap();
            info!("Ctrl-C received, shutting down");
            host.stop().await;
        }
        Err(e) => {
            error!("Failed to start host: {}", e);
        }
    }
    Ok(())
}

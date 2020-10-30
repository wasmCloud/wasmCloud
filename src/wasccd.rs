use actix_rt::System;
use lattice_cplane_nats::NatsControlPlaneProvider;
use lattice_rpc_nats::NatsLatticeProvider;
use wascc_host::HostBuilder;

#[macro_use]
extern crate log;

#[actix_rt::main]
async fn main() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().filter_or(
        env_logger::DEFAULT_FILTER_ENV,
        "wasccd=info,wascc_host=info",
    ))
    .format_module_path(false)
    .try_init();

    let host = HostBuilder::new().build();
    match host
        .start(
            None, //Some(Box::new(NatsLatticeProvider::new(None))),
            None, // Some(Box::new(NatsControlPlaneProvider::new())),
        )
        .await
    {
        Ok(_) => {
            actix_rt::signal::ctrl_c().await.unwrap();
            info!("Ctrl-C received, shutting down");
            host.stop().await;
        }
        Err(e) => {
            error!("Failed to start host: {}", e);
        }
    }
}

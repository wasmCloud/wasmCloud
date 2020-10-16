use actix_rt::System;
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
    match host.start(bus_nats::NatsBusProvider::new(None)).await {
        Ok(_) => {
            actix_rt::signal::ctrl_c().await.unwrap();
            info!("Ctrl-C received, shutting down");
            System::current().stop();
        }
        Err(e) => {
            error!("Failed to start host: {}", e);
        }
    }
}

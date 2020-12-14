use structopt::{clap::AppSettings, StructOpt};
use wasmcloud_host::{HostBuilder, Result};

#[macro_use]
extern crate log;

#[derive(StructOpt, Debug, Clone)]
#[structopt(
     global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
     name = "wasmcloud")]
struct Cli {
    /// Host for RPC connections
    #[structopt(long = "host", default_value = "0.0.0.0", env = "RPC_HOST")]
    rpc_host: String,

    /// Port for RPC connections
    #[structopt(long = "port", default_value = "4222", env = "RPC_PORT")]
    rpc_port: String,

    /// Allows live updating of actors
    #[structopt(long = "allow-live-updates")]
    allow_live_updates: bool,

    /// Allows the use of "latest" artifact tag
    #[structopt(long = "allow-oci-latest")]
    allow_oci_latest: bool,
}

#[actix_rt::main]
async fn main() -> Result<()> {
    let cli = Cli::from_args();
    let _ = env_logger::Builder::from_env(env_logger::Env::default().filter_or(
        env_logger::DEFAULT_FILTER_ENV,
        "wasmcloud=info,wasmcloud_host=info",
    ))
    .format_module_path(false)
    .try_init();

    let nats_url = &format!("{}:{}", cli.rpc_host, cli.rpc_port);
    let nc_rpc = nats::asynk::connect(nats_url).await?;
    let nc_control = nats::asynk::connect(nats_url).await?;

    let mut host_builder = HostBuilder::new()
        .with_rpc_client(nc_rpc)
        .with_control_client(nc_control);

    if cli.allow_live_updates {
        host_builder = host_builder.enable_live_updates();
    }
    if cli.allow_oci_latest {
        host_builder = host_builder.oci_allow_latest();
    }

    let host = host_builder.build();
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

use std::io::prelude::*;
use std::{fs::File, path::PathBuf};
use structopt::{clap::AppSettings, StructOpt};
use wasmcloud_host::{HostBuilder, HostManifest, Result};

#[macro_use]
extern crate log;

#[derive(StructOpt, Debug, Clone)]
#[structopt(
     global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
     name = "wasmcloud")]
struct Cli {
    /// Host for RPC connection
    #[structopt(long = "rpc-host", default_value = "0.0.0.0", env = "RPC_HOST")]
    rpc_host: String,

    /// Port for RPC connection
    #[structopt(long = "rpc-port", default_value = "4222", env = "RPC_PORT")]
    rpc_port: String,

    /// Host for control interface connection
    #[structopt(long = "control-host", default_value = "0.0.0.0", env = "CONTROL_HOST")]
    control_host: String,

    /// Port for control interface connection
    #[structopt(long = "control-port", default_value = "4222", env = "CONTROL_PORT")]
    control_port: String,

    /// JWT file for RPC authentication. Must be supplied with rpc_seed.
    #[structopt(long = "rpc-jwt", env = "RPC_JWT", hide_env_values = true)]
    rpc_jwt: Option<String>,

    /// Seed file or literal for RPC authentication. Must be supplied with rpc_jwt.
    #[structopt(long = "rpc-seed", env = "RPC_SEED", hide_env_values = true)]
    rpc_seed: Option<String>,

    /// Credsfile for RPC authentication
    #[structopt(long = "rpc-credsfile", env = "RPC_CREDS", hide_env_values = true)]
    rpc_credsfile: Option<String>,

    /// JWT file for control interface authentication. Must be supplied with control_seed.
    #[structopt(long = "control-jwt", env = "CONTROL_JWT", hide_env_values = true)]
    control_jwt: Option<String>,

    /// Seed file or literal for control interface authentication. Must be supplied with control_jwt.
    #[structopt(long = "control-seed", env = "CONTROL_SEED", hide_env_values = true)]
    control_seed: Option<String>,

    /// Credsfile for control interface authentication
    #[structopt(
        long = "control-credsfile",
        env = "CONTROL_CREDS",
        hide_env_values = true
    )]
    control_credsfile: Option<String>,

    /// Allows live updating of actors
    #[structopt(long = "allow-live-updates")]
    allow_live_updates: bool,

    /// Allows the use of "latest" artifact tag
    #[structopt(long = "allow-oci-latest")]
    allow_oci_latest: bool,

    /// Disables strict comparison of live updated actor claims
    #[structopt(long = "disable-strict-update-check")]
    disable_strict_update_check: bool,

    /// Allows the use of HTTP registry connections to these registries
    #[structopt(long = "allowed-insecure")]
    allowed_insecure: Vec<String>,

    /// Specifies a manifest file to apply to the host once started
    #[structopt(long = "manifest", short = "m", parse(from_os_str))]
    manifest: Option<PathBuf>,
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

    if let Some(ref pb) = cli.manifest {
        if !pb.exists() {
            error!("Specified manifest file {:?} could not be opened", pb);
            return Err("Manifest file could not be opened.".into());
        }
    }

    let nats_url = &format!("{}:{}", cli.rpc_host, cli.rpc_port);
    let nc_rpc = nats_connection(nats_url, cli.rpc_jwt, cli.rpc_seed, cli.rpc_credsfile).await?;
    let nc_control = nats_connection(
        nats_url,
        cli.control_jwt,
        cli.control_seed,
        cli.control_credsfile,
    )
    .await?;

    let mut host_builder = HostBuilder::new()
        .with_rpc_client(nc_rpc)
        .with_control_client(nc_control);

    if cli.allow_live_updates {
        host_builder = host_builder.enable_live_updates();
    }
    if cli.allow_oci_latest {
        host_builder = host_builder.oci_allow_latest();
    }
    if cli.disable_strict_update_check {
        host_builder = host_builder.disable_strict_update_check();
    }
    if !cli.allowed_insecure.is_empty() {
        host_builder = host_builder.oci_allow_insecure(cli.allowed_insecure);
    }

    let host = host_builder.build();
    match host.start().await {
        Ok(_) => {
            if let Some(pb) = cli.manifest {
                if pb.exists() {
                    let hm = HostManifest::from_path(pb, true)?;
                    host.apply_manifest(hm).await?;
                }
            }
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

async fn nats_connection(
    url: &str,
    jwt: Option<String>,
    seed: Option<String>,
    credsfile: Option<String>,
) -> Result<nats::asynk::Connection> {
    if let (Some(jwt_file), Some(seed_val)) = (jwt, seed) {
        let kp = nkeys::KeyPair::from_seed(&extract_arg_value(&seed_val)?)?;
        // You must provide the JWT via a closure
        Ok(nats::Options::with_jwt(
            move || Ok(jwt_file.clone()),
            move |nonce| kp.sign(nonce).unwrap(),
        )
        .connect_async(url)
        .await?)
    } else if let Some(credsfile_path) = credsfile {
        Ok(nats::Options::with_credentials(credsfile_path)
            .connect_async(url)
            .await?)
    } else {
        Ok(nats::asynk::connect(url).await?)
    }
}

/// Returns value from an argument that may be a file path or the value itself
fn extract_arg_value(arg: &str) -> Result<String> {
    match File::open(arg) {
        Ok(mut f) => {
            let mut value = String::new();
            f.read_to_string(&mut value)?;
            Ok(value)
        }
        Err(_) => Ok(arg.to_string()),
    }
}

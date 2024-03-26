//! Common config constants and functions for loading, finding, and consuming configuration data
use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use async_nats::Client;
use tokio::io::AsyncReadExt;
use wasmcloud_control_interface::{Client as CtlClient, ClientBuilder as CtlClientBuilder};

use crate::context::WashContext;

pub const WASH_DIR: &str = ".wash";

const DOWNLOADS_DIR: &str = "downloads";
pub const DEFAULT_NATS_HOST: &str = "127.0.0.1";
pub const DEFAULT_NATS_PORT: &str = "4222";
pub const DEFAULT_LATTICE: &str = "default";
pub const DEFAULT_NATS_TIMEOUT_MS: u64 = 2_000;
pub const DEFAULT_START_ACTOR_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_START_PROVIDER_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_CTX_DIR_NAME: &str = "contexts";

/// Get the path to the `.wash` configuration directory. Creates the directory if it does not exist.
pub fn cfg_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("no home directory found. Please set $HOME")?;

    let wash = home.join(WASH_DIR);

    if !wash.exists() {
        fs::create_dir_all(&wash)
            .with_context(|| format!("failed to create directory `{}`", wash.display()))?;
    }

    Ok(wash)
}

/// The path to the downloads directory for wash
pub fn downloads_dir() -> Result<PathBuf> {
    Ok(cfg_dir()?.join(DOWNLOADS_DIR))
}

#[derive(Clone)]
/// Connection options for a Wash instance
pub struct WashConnectionOptions {
    /// CTL Host for connection, defaults to 127.0.0.1 for local nats
    pub ctl_host: Option<String>,

    /// CTL Port for connections, defaults to 4222 for local nats
    pub ctl_port: Option<String>,

    /// JWT file for CTL authentication. Must be supplied with ctl_seed.
    pub ctl_jwt: Option<String>,

    /// Seed file or literal for CTL authentication. Must be supplied with ctl_jwt.
    pub ctl_seed: Option<String>,

    /// Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt.
    /// See https://docs.nats.io/using-nats/developer/connecting/creds for details.
    pub ctl_credsfile: Option<PathBuf>,

    /// JS domain for wasmcloud control interface. Defaults to None
    pub js_domain: Option<String>,

    /// Lattice name for wasmcloud control interface, defaults to "default"
    pub lattice: Option<String>,

    /// Timeout length to await a control interface response, defaults to 2000 milliseconds
    pub timeout_ms: u64,

    /// Wash context
    pub ctx: WashContext,
}

impl WashConnectionOptions {
    /// Create a control client from connection options
    pub async fn into_ctl_client(self, auction_timeout_ms: Option<u64>) -> Result<CtlClient> {
        let lattice = self.lattice.unwrap_or_else(|| self.ctx.lattice.clone());

        let ctl_host = self.ctl_host.unwrap_or_else(|| self.ctx.ctl_host.clone());
        let ctl_port = self
            .ctl_port
            .unwrap_or_else(|| self.ctx.ctl_port.to_string());
        let ctl_jwt = self.ctl_jwt.or_else(|| self.ctx.ctl_jwt.clone());
        let ctl_seed = self.ctl_seed.or_else(|| self.ctx.ctl_seed.clone());
        let ctl_credsfile = self
            .ctl_credsfile
            .or_else(|| self.ctx.ctl_credsfile.clone());

        let auction_timeout_ms = auction_timeout_ms.unwrap_or(self.timeout_ms);

        let nc =
            create_nats_client_from_opts(&ctl_host, &ctl_port, ctl_jwt, ctl_seed, ctl_credsfile)
                .await
                .context("Failed to create NATS client")?;

        let mut builder = CtlClientBuilder::new(nc)
            .lattice(lattice)
            .timeout(tokio::time::Duration::from_millis(self.timeout_ms))
            .auction_timeout(tokio::time::Duration::from_millis(auction_timeout_ms));

        if let Ok(topic_prefix) = std::env::var("WASMCLOUD_CTL_TOPIC_PREFIX") {
            builder = builder.topic_prefix(topic_prefix);
        }

        let ctl_client = builder.build();

        Ok(ctl_client)
    }

    /// Create a NATS client from `WashConnectionOptions`
    pub async fn into_nats_client(self) -> Result<Client> {
        let ctl_host = self.ctl_host.unwrap_or_else(|| self.ctx.ctl_host.clone());
        let ctl_port = self
            .ctl_port
            .unwrap_or_else(|| self.ctx.ctl_port.to_string());
        let ctl_jwt = self.ctl_jwt.or_else(|| self.ctx.ctl_jwt.clone());
        let ctl_seed = self.ctl_seed.or_else(|| self.ctx.ctl_seed.clone());
        let ctl_credsfile = self
            .ctl_credsfile
            .or_else(|| self.ctx.ctl_credsfile.clone());

        let nc =
            create_nats_client_from_opts(&ctl_host, &ctl_port, ctl_jwt, ctl_seed, ctl_credsfile)
                .await?;

        Ok(nc)
    }

    /// Either returns the opts.lattice or opts.ctx.lattice... if both are absent/None,  returns the default lattice prefix (`DEFAULT_LATTICE`).
    #[must_use]
    pub fn get_lattice(&self) -> String {
        self.lattice
            .clone()
            .unwrap_or_else(|| self.ctx.lattice.clone())
    }
}

/// Reads the content of a string if it is a valid file path, otherwise returning the string
async fn extract_arg_value(arg: &str) -> Result<String> {
    match tokio::fs::File::open(arg).await {
        Ok(mut f) => {
            let mut value = String::new();
            f.read_to_string(&mut value)
                .await
                .with_context(|| format!("Failed to read file {}", &arg))?;
            Ok(value)
        }
        Err(_) => Ok(arg.into()),
    }
}

/// Create a NATS client from NATS-related options
pub async fn create_nats_client_from_opts(
    host: &str,
    port: &str,
    jwt: Option<String>,
    seed: Option<String>,
    credsfile: Option<PathBuf>,
) -> Result<async_nats::Client> {
    let nats_url = format!("{host}:{port}");
    use async_nats::ConnectOptions;

    let nc = if let Some(jwt_file) = jwt {
        let jwt_contents = extract_arg_value(&jwt_file)
            .await
            .with_context(|| format!("Failed to extract jwt contents from {}", &jwt_file))?;
        let kp = std::sync::Arc::new(if let Some(seed) = seed {
            nkeys::KeyPair::from_seed(
                &extract_arg_value(&seed)
                    .await
                    .with_context(|| format!("Failed to extract seed value {}", &seed))?,
            )
            .with_context(|| format!("Failed to create keypair from seed value {}", &seed))?
        } else {
            nkeys::KeyPair::new_user()
        });

        // You must provide the JWT via a closure
        async_nats::ConnectOptions::with_jwt(jwt_contents, move |nonce| {
            let key_pair = kp.clone();
            async move { key_pair.sign(&nonce).map_err(async_nats::AuthError::new) }
        })
        .connect(&nats_url)
        .await
        .with_context(|| {
            format!(
                "Failed to connect to NATS server {}:{} while creating client",
                &host, &port
            )
        })?
    } else if let Some(credsfile_path) = credsfile {
        ConnectOptions::with_credentials_file(credsfile_path.clone())
            .await
            .with_context(|| {
                format!(
                    "Failed to authenticate to NATS with credentials file {:?}",
                    &credsfile_path
                )
            })?
            .connect(&nats_url)
            .await
            .with_context(|| {
                format!(
                    "Failed to connect to NATS {} with credentials file {:?}",
                    &nats_url, &credsfile_path
                )
            })?
    } else {
        async_nats::connect(&nats_url).await.with_context(|| format!("Failed to connect to NATS {}\nNo credentials file was provided, you may need one to connect.", &nats_url))?
    };
    Ok(nc)
}

//! Common config constants and functions for loading, finding, and consuming configuration data
use std::{
    fs,
    io::{Error, ErrorKind, Result as IoResult},
    path::PathBuf,
};

use anyhow::{Context, Result};
use async_nats::Client;
use tokio::io::AsyncReadExt;
use wasmcloud_control_interface::{Client as CtlClient, ClientBuilder as CtlClientBuilder};

use crate::context::WashContext;

pub const WASH_DIR: &str = ".wash";

const DOWNLOADS_DIR: &str = "downloads";
pub const DEFAULT_NATS_HOST: &str = "127.0.0.1";
pub const DEFAULT_NATS_PORT: &str = "4222";
pub const DEFAULT_LATTICE_PREFIX: &str = "default";
pub const DEFAULT_NATS_TIMEOUT_MS: u64 = 2_000;
pub const DEFAULT_START_ACTOR_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_START_PROVIDER_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_CTX_DIR_NAME: &str = "contexts";

/// Get the path to the `.wash` configuration directory. Creates the directory if it does not exist.
pub fn cfg_dir() -> IoResult<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| {
        Error::new(
            ErrorKind::NotFound,
            "No home directory found. Please set $HOME.",
        )
    })?;

    let wash = home.join(WASH_DIR);

    if !wash.exists() {
        fs::create_dir_all(&wash)?;
    }

    Ok(wash)
}

/// Given an optional supplied directory, determine the context directory either from the supplied
/// directory or using the home directory and the predefined `.wash/contexts` folder.
pub fn context_dir(cmd_dir: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(dir) = cmd_dir {
        Ok(dir)
    } else {
        Ok(cfg_dir()?.join(DEFAULT_CTX_DIR_NAME))
    }
}

/// Returns the path to the caching directory for smithy files
pub fn model_cache_dir() -> IoResult<PathBuf> {
    weld_codegen::weld_cache_dir().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
}

/// The path to the downloads directory for wash
pub fn downloads_dir() -> IoResult<PathBuf> {
    cfg_dir().map(|p| p.join(DOWNLOADS_DIR))
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
    /// See https://docs.nats.io/developing-with-nats/security/creds for details.
    pub ctl_credsfile: Option<PathBuf>,

    /// JS domain for wasmcloud control interface. Defaults to None
    pub js_domain: Option<String>,

    /// Lattice prefix for wasmcloud control interface, defaults to "default"
    pub lattice_prefix: Option<String>,

    /// Timeout length to await a control interface response, defaults to 2000 milliseconds
    pub timeout_ms: u64,

    /// Wash context
    pub ctx: Option<WashContext>,
}

impl WashConnectionOptions {
    /// Create a control client from connection options
    pub async fn into_ctl_client(self, auction_timeout_ms: Option<u64>) -> Result<CtlClient> {
        let lattice_prefix = self.lattice_prefix.unwrap_or_else(|| {
            self.ctx
                .as_ref()
                .map(|c| c.lattice_prefix.clone())
                .unwrap_or_else(|| DEFAULT_LATTICE_PREFIX.to_string())
        });

        let ctl_host = self.ctl_host.unwrap_or_else(|| {
            self.ctx
                .as_ref()
                .map(|c| c.ctl_host.clone())
                .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string())
        });

        let ctl_port = self.ctl_port.unwrap_or_else(|| {
            self.ctx
                .as_ref()
                .map(|c| c.ctl_port.to_string())
                .unwrap_or_else(|| DEFAULT_NATS_PORT.to_string())
        });

        let ctl_jwt = self.ctl_jwt.or_else(|| {
            self.ctx
                .as_ref()
                .map(|c| c.ctl_jwt.clone())
                .unwrap_or_default()
        });

        let ctl_seed = self.ctl_seed.or_else(|| {
            self.ctx
                .as_ref()
                .map(|c| c.ctl_seed.clone())
                .unwrap_or_default()
        });

        let ctl_credsfile = self.ctl_credsfile.or_else(|| {
            self.ctx
                .as_ref()
                .map(|c| c.ctl_credsfile.clone())
                .unwrap_or_default()
        });

        let auction_timeout_ms = auction_timeout_ms.unwrap_or(self.timeout_ms);

        let nc =
            create_nats_client_from_opts(&ctl_host, &ctl_port, ctl_jwt, ctl_seed, ctl_credsfile)
                .await
                .context("Failed to create NATS client")?;

        let mut builder = CtlClientBuilder::new(nc)
            .lattice_prefix(lattice_prefix)
            .timeout(tokio::time::Duration::from_millis(self.timeout_ms))
            .auction_timeout(tokio::time::Duration::from_millis(auction_timeout_ms));

        if let Ok(topic_prefix) = std::env::var("WASMCLOUD_CTL_TOPIC_PREFIX") {
            builder = builder.topic_prefix(topic_prefix);
        }

        let ctl_client = builder.build();

        Ok(ctl_client)
    }

    /// Create a NATS client from WashConnectionOptions
    pub async fn into_nats_client(self) -> Result<Client> {
        let ctl_host = self.ctl_host.unwrap_or_else(|| {
            self.ctx
                .as_ref()
                .map(|c| c.ctl_host.clone())
                .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string())
        });

        let ctl_port = self.ctl_port.unwrap_or_else(|| {
            self.ctx
                .as_ref()
                .map(|c| c.ctl_port.to_string())
                .unwrap_or_else(|| DEFAULT_NATS_PORT.to_string())
        });

        let ctl_jwt = if self.ctl_jwt.is_some() {
            self.ctl_jwt
        } else {
            self.ctx
                .as_ref()
                .map(|c| c.ctl_jwt.clone())
                .unwrap_or_default()
        };

        let ctl_seed = if self.ctl_seed.is_some() {
            self.ctl_seed
        } else {
            self.ctx
                .as_ref()
                .map(|c| c.ctl_seed.clone())
                .unwrap_or_default()
        };

        let ctl_credsfile = if self.ctl_credsfile.is_some() {
            self.ctl_credsfile
        } else {
            self.ctx
                .as_ref()
                .map(|c| c.ctl_credsfile.clone())
                .unwrap_or_default()
        };

        let nc =
            create_nats_client_from_opts(&ctl_host, &ctl_port, ctl_jwt, ctl_seed, ctl_credsfile)
                .await?;

        Ok(nc)
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
async fn create_nats_client_from_opts(
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

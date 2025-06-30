//! Common config constants and functions for loading, finding, and consuming configuration data
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock as Lazy;

use anyhow::{Context, Result};
use async_nats::Client;
use etcetera::{app_strategy, AppStrategy, AppStrategyArgs};
use tokio::io::AsyncReadExt;
use wasmcloud_control_interface::{Client as CtlClient, ClientBuilder as CtlClientBuilder};

use crate::lib::context::WashContext;

pub const WASMCLOUD_PID_FILE: &str = "wasmcloud.pid";
pub const WADM_PID_FILE: &str = "wadm.pid";
pub const DEFAULT_NATS_HOST: &str = "127.0.0.1";
pub const DEFAULT_NATS_PORT: &str = "4222";
pub const DEFAULT_LATTICE: &str = "default";
pub const DEFAULT_NATS_TIMEOUT_MS: u64 = 2_000;
pub const DEFAULT_START_COMPONENT_TIMEOUT_MS: u64 = 10_000;
pub const DEFAULT_COMPONENT_OPERATION_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_START_PROVIDER_TIMEOUT_MS: u64 = 60_000;

pub static WASH_DIRECTORIES: Lazy<WashAppStrategy> = Lazy::new(|| {
    let args = AppStrategyArgs {
        top_level_domain: "com".to_string(),
        author: "wasmCloud".to_string(),
        app_name: "wash".to_string(),
    };

    let strategy = app_strategy::choose_app_strategy(args).expect("failed to get AppStrategy");
    #[cfg(target_os = "windows")]
    {
        WashAppStrategy::Windows(strategy)
    }
    #[cfg(not(target_os = "windows"))]
    {
        WashAppStrategy::Xdg(strategy)
    }
});

pub enum WashAppStrategy {
    Xdg(app_strategy::Xdg),
    Windows(app_strategy::Windows),
}

impl AppStrategy for WashAppStrategy {
    fn home_dir(&self) -> &std::path::Path {
        match self {
            Self::Xdg(s) => s.home_dir(),
            Self::Windows(s) => s.home_dir(),
        }
    }

    fn config_dir(&self) -> PathBuf {
        var_path("WASH_CONFIG_DIR").unwrap_or_else(|_| match self {
            Self::Xdg(s) => s.config_dir(),
            Self::Windows(s) => s.config_dir(),
        })
    }

    fn data_dir(&self) -> PathBuf {
        var_path("WASH_DATA_DIR").unwrap_or_else(|_| match self {
            Self::Xdg(s) => s.data_dir(),
            Self::Windows(s) => s.data_dir(),
        })
    }

    fn cache_dir(&self) -> PathBuf {
        var_path("WASH_CACHE_DIR").unwrap_or_else(|_| match self {
            Self::Xdg(s) => s.cache_dir(),
            Self::Windows(s) => s.cache_dir(),
        })
    }

    fn state_dir(&self) -> Option<PathBuf> {
        match self {
            Self::Xdg(s) => s.state_dir(),
            Self::Windows(s) => s.state_dir(),
        }
    }

    fn runtime_dir(&self) -> Option<PathBuf> {
        match self {
            Self::Xdg(s) => s.runtime_dir(),
            Self::Windows(s) => s.runtime_dir(),
        }
    }
}

impl WashAppStrategy {
    /// Creates the directory in which to store configuration in and returns the path to it.
    pub fn create_config_dir(&self) -> Result<PathBuf> {
        let path = self.config_dir();
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create directory `{}`", path.display()))?;
        Ok(path)
    }

    /// Creates the directory in which to store configuration in and returns the path to it
    /// concatenated with a filename.
    pub fn create_in_config_dir<P: AsRef<OsStr>>(&self, path: P) -> Result<PathBuf> {
        self.create_config_dir()?;
        Ok(self.in_config_dir(path))
    }

    /// Returns the path to the directory where to store keys.
    pub fn keys_dir(&self) -> PathBuf {
        var_path("WASH_KEYS_DIR").unwrap_or_else(|_| self.in_data_dir("keys"))
    }

    /// Concatenates a path in the keys directory.
    pub fn in_keys_dir<P: AsRef<std::ffi::OsStr>>(&self, path: P) -> PathBuf {
        let mut basepath = self.keys_dir();
        basepath.push(Path::new(&path));
        basepath
    }

    /// Creates the directory in which to store keys in and returns the path to it.
    pub fn create_keys_dir(&self) -> Result<PathBuf> {
        let path = self.keys_dir();
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create directory `{}`", path.display()))?;
        Ok(path)
    }

    /// Creates the directory in which to store keys in and returns the path to it
    /// concatenated with a filename.
    pub fn create_in_keys_dir<P: AsRef<OsStr>>(&self, path: P) -> Result<PathBuf> {
        self.create_keys_dir()?;
        Ok(self.in_keys_dir(path))
    }

    /// Returns the path to the directory where to store logs.
    pub fn logs_dir(&self) -> PathBuf {
        var_path("WASH_LOGS_DIR").unwrap_or_else(|_| WASH_DIRECTORIES.in_data_dir("logs"))
    }

    /// Concatenates a path in the logs directory.
    pub fn in_logs_dir<P: AsRef<std::ffi::OsStr>>(&self, path: P) -> PathBuf {
        let mut basepath = self.logs_dir();
        basepath.push(Path::new(&path));
        basepath
    }

    /// Creates the directory in which to store logs in and returns the path to it.
    pub fn create_logs_dir(&self) -> Result<PathBuf> {
        let path = self.logs_dir();
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create directory `{}`", path.display()))?;
        Ok(path)
    }

    /// Creates the directory in which to store logs in and returns the path to it
    /// concatenated with a filename.
    pub fn create_in_logs_dir<P: AsRef<OsStr>>(&self, path: P) -> Result<PathBuf> {
        self.create_logs_dir()?;
        Ok(self.in_logs_dir(path))
    }

    /// Returns the path to the directory where to store downloads.
    pub fn downloads_dir(&self) -> PathBuf {
        var_path("WASH_DOWNLOADS_DIR").unwrap_or_else(|_| WASH_DIRECTORIES.in_data_dir("downloads"))
    }

    /// Concatenates a path in the downloads directory.
    pub fn in_downloads_dir<P: AsRef<std::ffi::OsStr>>(&self, path: P) -> PathBuf {
        let mut basepath = self.downloads_dir();
        basepath.push(Path::new(&path));
        basepath
    }

    /// Creates the directory in which to store downloads in and returns the path to it.
    pub fn create_downloads_dir(&self) -> Result<PathBuf> {
        let path = self.downloads_dir();
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create directory `{}`", path.display()))?;
        Ok(path)
    }

    /// Creates the directory in which to store downloads in and returns the path to it
    /// concatenated with a filename.
    pub fn create_in_downloads_dir<P: AsRef<OsStr>>(&self, path: P) -> Result<PathBuf> {
        self.create_downloads_dir()?;
        Ok(self.in_downloads_dir(path))
    }

    /// Returns the path to the directory where to store plugins.
    pub fn plugins_dir(&self) -> PathBuf {
        var_path("WASH_PLUGINS_DIR").unwrap_or_else(|_| WASH_DIRECTORIES.in_data_dir("plugins"))
    }

    /// Concatenates a path in the plugins directory.
    pub fn in_plugins_dir<P: AsRef<std::ffi::OsStr>>(&self, path: P) -> PathBuf {
        let mut basepath = self.plugins_dir();
        basepath.push(Path::new(&path));
        basepath
    }

    /// Creates the directory in which to store plugins in and returns the path to it.
    pub fn create_plugins_dir(&self) -> Result<PathBuf> {
        let path = self.plugins_dir();
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create directory `{}`", path.display()))?;
        Ok(path)
    }

    /// Creates the directory in which to store plugins in and returns the path to it
    /// concatenated with a filename.
    pub fn create_in_plugins_dir<P: AsRef<OsStr>>(&self, path: P) -> Result<PathBuf> {
        self.create_plugins_dir()?;
        Ok(self.in_plugins_dir(path))
    }

    /// Returns the path to the directory where to store the package cache.
    pub fn package_cache_dir(&self) -> PathBuf {
        var_path("WASH_PACKAGE_CACHE_DIR")
            .unwrap_or_else(|_| WASH_DIRECTORIES.in_cache_dir("package_cache"))
    }

    /// Concatenates a path in the package cache directory.
    pub fn in_package_cache_dir<P: AsRef<std::ffi::OsStr>>(&self, path: P) -> PathBuf {
        let mut basepath = self.package_cache_dir();
        basepath.push(Path::new(&path));
        basepath
    }

    /// Creates the directory in which to store cached packages in and returns the path to it.
    pub fn create_package_cache_dir(&self) -> Result<PathBuf> {
        let path = self.package_cache_dir();
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create directory `{}`", path.display()))?;
        Ok(path)
    }

    /// Creates the directory in which to store the package cache in and returns the path to it
    /// concatenated with a filename.
    pub fn create_in_package_cache_dir<P: AsRef<OsStr>>(&self, path: P) -> Result<PathBuf> {
        self.create_package_cache_dir()?;
        Ok(self.in_package_cache_dir(path))
    }

    /// Returns the path to the directory where to store the development stuff.
    pub fn dev_dir(&self) -> PathBuf {
        var_path("WASH_DEV_DIR").unwrap_or_else(|_| WASH_DIRECTORIES.in_data_dir("dev"))
    }

    /// Concatenates a path in the development directory.
    pub fn in_dev_dir<P: AsRef<std::ffi::OsStr>>(&self, path: P) -> PathBuf {
        let mut basepath = self.dev_dir();
        basepath.push(Path::new(&path));
        basepath
    }

    /// Creates the directory in which to store dev stuff in and returns the path to it.
    pub fn create_dev_dir(&self) -> Result<PathBuf> {
        let path = self.dev_dir();
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create directory `{}`", path.display()))?;
        Ok(path)
    }

    /// Creates the directory in which to store dev in and returns the path to it
    /// concatenated with a filename.
    pub fn create_in_dev_dir<P: AsRef<OsStr>>(&self, path: P) -> Result<PathBuf> {
        self.create_dev_dir()?;
        Ok(self.in_dev_dir(path))
    }

    /// Returns the path to the directory where to store contexts.
    pub fn context_dir(&self) -> PathBuf {
        var_path("WASH_CONTEXT_DIR").unwrap_or_else(|_| WASH_DIRECTORIES.in_config_dir("contexts"))
    }

    /// Concatenates a path in the context directory.
    pub fn in_context_dir<P: AsRef<std::ffi::OsStr>>(&self, path: P) -> PathBuf {
        let mut basepath = self.context_dir();
        basepath.push(Path::new(&path));
        basepath
    }

    /// Creates the directory in which to store contexts in and returns the path to it.
    pub fn create_context_dir(&self) -> Result<PathBuf> {
        let path = self.context_dir();
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create directory `{}`", path.display()))?;
        Ok(path)
    }

    /// Creates the directory in which to store context in and returns the path to it
    /// concatenated with a filename.
    pub fn create_in_context_dir<P: AsRef<OsStr>>(&self, path: P) -> Result<PathBuf> {
        self.create_context_dir()?;
        Ok(self.in_context_dir(path))
    }
}

fn var_path(key: &str) -> Result<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .context("variable not found")
        .and_then(replace_path)
}

fn replace_path<P: AsRef<Path>>(path: P) -> Result<PathBuf> {
    let path = path.as_ref();
    Ok(match path.starts_with("~/") {
        true => {
            let home = etcetera::home_dir().context("no home directory found. Please set $HOME")?;
            home.join(path.strip_prefix("~/").unwrap())
        }
        false => path.to_path_buf(),
    })
}

/// The path to the running wasmCloud Host PID file for wash
pub fn host_pid_file() -> Result<PathBuf> {
    WASH_DIRECTORIES.create_in_downloads_dir(WASMCLOUD_PID_FILE)
}

/// The path to the running wadm PID file for wash
pub fn wadm_pid_file() -> Result<PathBuf> {
    WASH_DIRECTORIES.create_in_downloads_dir(WADM_PID_FILE)
}

#[derive(Clone, Default)]
/// Connection options for a Wash instance
pub struct WashConnectionOptions {
    /// CTL Host for connection, defaults to 127.0.0.1 for local nats
    pub ctl_host: Option<String>,

    /// CTL Port for connections, defaults to 4222 for local nats
    pub ctl_port: Option<String>,

    /// JWT file for CTL authentication. Must be supplied with `ctl_seed`.
    pub ctl_jwt: Option<String>,

    /// Seed file or literal for CTL authentication. Must be supplied with `ctl_jwt`.
    pub ctl_seed: Option<String>,

    /// Credsfile for CTL authentication. Combines `ctl_seed` and `ctl_jwt`.
    /// See [the NATS documentation](https://docs.nats.io/using-nats/developer/connecting/creds) for details.
    pub ctl_credsfile: Option<PathBuf>,

    /// Path to a file containing a CA certificate to use for TLS connections
    pub ctl_tls_ca_file: Option<PathBuf>,

    /// Perform TLS handshake before expecting the server greeting.
    pub ctl_tls_first: Option<bool>,

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
        let ctl_tls_ca_file = self
            .ctl_tls_ca_file
            .or_else(|| self.ctx.ctl_tls_ca_file.clone());
        let ctl_tls_first = self
            .ctl_tls_first
            .unwrap_or_else(|| self.ctx.ctl_tls_first.unwrap_or(false));
        let auction_timeout_ms = auction_timeout_ms.unwrap_or(self.timeout_ms);

        let nc = create_nats_client_from_opts(
            &ctl_host,
            &ctl_port,
            ctl_jwt,
            ctl_seed,
            ctl_credsfile,
            ctl_tls_ca_file,
            ctl_tls_first,
        )
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
        let ctl_tls_ca_file = self
            .ctl_tls_ca_file
            .or_else(|| self.ctx.ctl_tls_ca_file.clone());
        let ctl_tls_first = self
            .ctl_tls_first
            .unwrap_or_else(|| self.ctx.ctl_tls_first.unwrap_or(false));
        let nc = create_nats_client_from_opts(
            &ctl_host,
            &ctl_port,
            ctl_jwt,
            ctl_seed,
            ctl_credsfile,
            ctl_tls_ca_file,
            ctl_tls_first,
        )
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
    tls_ca_file: Option<PathBuf>,
    tls_first: bool,
) -> Result<Client> {
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
        let mut opts = async_nats::ConnectOptions::with_jwt(jwt_contents, move |nonce| {
            let key_pair = kp.clone();
            async move { key_pair.sign(&nonce).map_err(async_nats::AuthError::new) }
        });

        if let Some(ca_file) = tls_ca_file {
            opts = opts.add_root_certificates(ca_file).require_tls(true);
        }

        if tls_first {
            opts = opts.tls_first();
        }

        opts.name("wash-lib")
            .connect(&nats_url)
            .await
            .with_context(|| {
                format!(
                    "Failed to connect to NATS server {}:{} while creating client",
                    &host, &port
                )
            })?
    } else if let Some(credsfile_path) = credsfile {
        let mut opts = ConnectOptions::with_credentials_file(credsfile_path.clone())
            .await
            .with_context(|| {
                format!(
                    "Failed to authenticate to NATS with credentials file {:?}",
                    &credsfile_path
                )
            })?;

        if let Some(ca_file) = tls_ca_file {
            opts = opts.add_root_certificates(ca_file).require_tls(true);
        }

        if tls_first {
            opts = opts.tls_first();
        }

        opts.name("wash-lib")
            .connect(&nats_url)
            .await
            .with_context(|| {
                format!(
                    "Failed to connect to NATS {} with credentials file {:?}",
                    &nats_url, &credsfile_path
                )
            })?
    } else {
        let mut opts = ConnectOptions::new();

        if let Some(ca_file) = tls_ca_file {
            opts = opts.add_root_certificates(ca_file).require_tls(true);
        }

        if tls_first {
            opts = opts.tls_first();
        }

        opts.name("wash-lib")
            .connect(&nats_url)
            .await
            .with_context(|| format!("Failed to connect to NATS {}", &nats_url))?
    };
    Ok(nc)
}

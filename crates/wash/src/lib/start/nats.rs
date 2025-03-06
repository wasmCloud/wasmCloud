use anyhow::{bail, Result};
use command_group::AsyncCommandGroup;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::fs::{metadata, write};
use tokio::process::{Child, Command};
use tracing::warn;

use crate::lib::common::CommandGroupUsage;
use crate::lib::start::wait_for_server;

use super::download_binary_from_github;

const NATS_GITHUB_RELEASE_URL: &str = "https://github.com/nats-io/nats-server/releases/download";
pub const NATS_SERVER_CONF: &str = "nats.conf";
pub const NATS_SERVER_PID: &str = "nats.pid";
#[cfg(target_family = "unix")]
pub const NATS_SERVER_BINARY: &str = "nats-server";
#[cfg(target_family = "windows")]
pub const NATS_SERVER_BINARY: &str = "nats-server.exe";

/// Downloads the NATS binary for the architecture and operating system of the current host machine.
///
/// # Arguments
///
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to download the `nats-server` binary to
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use crate::lib::start::ensure_nats_server;
/// let res = ensure_nats_server("v2.10.7", "/tmp/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/nats-server");
/// # }
/// ```
pub async fn ensure_nats_server<P>(version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    ensure_nats_server_for_os_arch_pair(std::env::consts::OS, std::env::consts::ARCH, version, dir)
        .await
}

/// Ensures the `nats-server` binary is installed, returning the path to the executable early if it exists or
/// downloading the specified GitHub release version of nats-server from <https://github.com/nats-io/nats-server/releases/>
/// and unpacking the binary for a specified OS/ARCH pair to a directory. Returns the path to the NATS executable.
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to download the `nats-server` binary to
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use crate::lib::start::ensure_nats_server_for_os_arch_pair;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = ensure_nats_server_for_os_arch_pair(os, arch, "v2.10.7", "/tmp/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/nats-server");
/// # }
/// ```
pub async fn ensure_nats_server_for_os_arch_pair<P>(
    os: &str,
    arch: &str,
    version: &str,
    dir: P,
) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let nats_bin_path = dir.as_ref().join(NATS_SERVER_BINARY);
    if let Ok(_md) = metadata(&nats_bin_path).await {
        // Check version to see if we need to update
        if let Ok(output) = Command::new(&nats_bin_path).arg("version").output().await {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            eprintln!(
                "👀 Found nats-server version on the disk: {}",
                stdout.trim_end()
            );
            let re = regex::Regex::new(r"^nats-server:[^\s]*").unwrap();
            if re.replace(&stdout, "").to_string().trim() == version {
                // nats-server already at correct version, return early
                eprintln!("✅ Using nats-server version [{}]", &version);
                return Ok(nats_bin_path);
            }
        }
    }

    eprintln!(
        "🎣 Downloading new nats-server from {}",
        &nats_url(os, arch, version)
    );

    // Download NATS binary
    let res =
        download_binary_from_github(&nats_url(os, arch, version), dir, NATS_SERVER_BINARY).await;
    if let Ok(ref path) = res {
        eprintln!("🎯 Saved nats-server to {}", path.display());
    }

    res
}

/// Downloads the NATS binary for the architecture and operating system of the current host machine.
///
/// # Arguments
///
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to download the `nats-server` binary to
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use crate::lib::start::download_nats_server;
/// let res = download_nats_server("v2.10.7", "/tmp/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/nats-server");
/// # }
/// ```
pub async fn download_nats_server<P>(version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    download_binary_from_github(
        &nats_url(std::env::consts::OS, std::env::consts::ARCH, version),
        dir,
        NATS_SERVER_BINARY,
    )
    .await
}

/// Configuration for a NATS server that supports running either in "standalone" or "leaf" mode.
///
/// See the respective [`NatsConfig::new_standalone`] and [`NatsConfig::new_leaf`] implementations below for more information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NatsConfig {
    pub host: String,
    pub port: u16,
    /// The path where the NATS server will store its jetstream data. This must be different for
    /// each NATS server you spin up, otherwise they will share stream data
    pub store_dir: PathBuf,
    pub js_domain: Option<String>,
    pub remote_url: Option<String>,
    pub credentials: Option<PathBuf>,
    pub websocket_port: u16,
    pub config_path: Option<PathBuf>,
}

/// Returns a standalone NATS config with the following values:
/// * `host`: `127.0.0.1`
/// * `port`: `4222`
/// * `js_domain`: `Some("core")`
/// * `remote_url`: `None`
/// * `credentials`: `None`
/// * `websocket_port`: `4223`
impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 4222,
            store_dir: std::env::temp_dir().join("wash-jetstream-4222"),
            js_domain: Some("core".to_string()),
            remote_url: None,
            credentials: None,
            websocket_port: 4223,
            config_path: None,
        }
    }
}

impl NatsConfig {
    /// Instantiates config for a NATS leaf node. Leaf nodes are meant to extend
    /// an existing NATS infrastructure like [Synadia's NGS](https://synadia.com/ngs), but can
    /// also be used to extend your own NATS infrastructure. For more information,
    /// our [Working with Leaf Nodes](https://wasmcloud.dev/reference/lattice/leaf-nodes/) docs
    ///
    /// # Arguments
    /// * `host`: NATS host to listen on, e.g. `127.0.0.1`
    /// * `port`: NATS port to listen on, e.g. `4222`
    /// * `js_domain`: Jetstream domain to use, defaults to `core`. See [Configuring Jetstream](https://wasmcloud.dev/reference/lattice/jetstream/) for more information
    /// * `remote_url`: URL of NATS cluster to extend
    /// * `credentials`: Credentials to authenticate to the existing NATS cluster
    #[must_use]
    pub fn new_leaf(
        host: &str,
        port: u16,
        js_domain: Option<String>,
        remote_url: String,
        credentials: PathBuf,
        websocket_port: u16,
        config_path: Option<PathBuf>,
    ) -> Self {
        Self {
            host: host.to_owned(),
            port,
            store_dir: std::env::temp_dir().join(format!("wash-jetstream-{port}")),
            js_domain,
            remote_url: Some(remote_url),
            credentials: Some(credentials),
            websocket_port,
            config_path,
        }
    }
    /// Instantiates config for a standalone NATS server. Unless you're looking to extend
    /// existing NATS infrastructure, this is the preferred NATS server mode.
    ///
    /// # Arguments
    /// * `host`: NATS host to listen on, e.g. `127.0.0.1`
    /// * `port`: NATS port to listen on, e.g. `4222`
    /// * `js_domain`: Jetstream domain to use, defaults to `core`. See [Configuring Jetstream](https://wasmcloud.dev/reference/lattice/jetstream/) for more information
    pub fn new_standalone(host: &str, port: u16, js_domain: Option<String>) -> Self {
        if host == "0.0.0.0" {
            warn!("Listening on 0.0.0.0 is unsupported on some platforms, use 127.0.0.1 for best results");
        }
        Self {
            host: host.to_owned(),
            port,
            store_dir: std::env::temp_dir().join(format!("wash-jetstream-{port}")),
            js_domain,
            ..Default::default()
        }
    }

    pub async fn write_to_path<P>(self, path: P) -> Result<()>
    where
        P: AsRef<Path>,
    {
        let leafnode_section = if let Some(url) = self.remote_url {
            let url_line = format!(r#"url: "{url}""#);
            let creds_line = self
                .credentials
                .as_ref()
                .map(|c| format!("credentials: {c:?}"))
                .unwrap_or_default();

            format!(
                r"
leafnodes {{
    remotes = [
        {{
            {url_line}
            {creds_line}
        }}
    ]
}}
                ",
            )
        } else {
            String::new()
        };
        let websocket_port = self.websocket_port;
        let websocket_section = format!(
            r"
websocket {{
    port: {websocket_port}
    no_tls: true
}}
                "
        );
        let config = format!(
            r"
jetstream {{
    domain={}
    store_dir={:?}
}}
{leafnode_section}
{websocket_section}
",
            self.js_domain.unwrap_or_else(|| "core".to_string()),
            self.store_dir.as_os_str().to_string_lossy()
        );
        write(path, config).await.map_err(anyhow::Error::from)
    }
}

/// Helper function to execute a NATS server binary with required wasmCloud arguments, e.g. `JetStream`
/// # Arguments
///
/// * `bin_path` - Path to the nats-server binary to execute
/// * `stderr` - Specify where NATS stderr logs should be written to. If logs aren't important, use `std::process::Stdio::null()`
/// * `config` - Configuration for the NATS server, see [`NatsConfig`] for options. This config file is written alongside the nats-server binary as `nats.conf`
pub async fn start_nats_server<P, T>(
    bin_path: P,
    stderr: T,
    config: NatsConfig,
    command_group: CommandGroupUsage,
) -> Result<Child>
where
    P: AsRef<Path>,
    T: Into<Stdio>,
{
    let host_addr = format!("{}:{}", config.host, config.port);

    // If we can connect to the local port, NATS won't be able to listen on that port
    if tokio::net::TcpStream::connect(&host_addr).await.is_ok() {
        bail!(
            "could not start NATS server, a process is already listening on {}:{}",
            config.host,
            config.port
        );
    }

    let bin_path_ref = bin_path.as_ref();

    let Some(parent_path) = bin_path_ref.parent() else {
        bail!("could not write config to disk, couldn't find download directory")
    };

    let config_path = parent_path.join(NATS_SERVER_CONF);
    let host = config.host.clone();
    let port = config.port;

    let mut cmd_args = vec![
        "-js".to_string(),
        "--addr".to_string(),
        host,
        "--port".to_string(),
        port.to_string(),
        "--pid".to_string(),
        parent_path
            .join(NATS_SERVER_PID)
            .to_string_lossy()
            .to_string(),
        "--config".to_string(),
    ];

    if let Some(nats_cfg_path) = &config.config_path {
        anyhow::ensure!(
            nats_cfg_path.is_file(),
            "The provided NATS config File [{:?}] is not a valid File",
            nats_cfg_path
        );

        cmd_args.push(nats_cfg_path.to_string_lossy().to_string());
    } else {
        config.write_to_path(&config_path).await?;
        cmd_args.push(config_path.to_string_lossy().to_string());
    }

    let mut cmd = Command::new(bin_path_ref);
    cmd.stderr(stderr.into())
        .stdin(Stdio::null())
        .args(&cmd_args);
    let child = if command_group == CommandGroupUsage::CreateNew {
        cmd.group_spawn().map_err(anyhow::Error::from)?.into_inner()
    } else {
        cmd.spawn().map_err(anyhow::Error::from)?
    };

    wait_for_server(&host_addr, "NATS server")
        .await
        .map(|()| child)
}

/// Helper function to get the path to the NATS server pid file
pub fn nats_pid_path<P>(install_dir: P) -> PathBuf
where
    P: AsRef<Path>,
{
    install_dir.as_ref().join(NATS_SERVER_PID)
}

/// Helper function to determine the NATS server release path given an os/arch and version
fn nats_url(os: &str, arch: &str, version: &str) -> String {
    // Replace "macos" with "darwin" to match NATS release scheme
    let os = if os == "macos" { "darwin" } else { os };
    // Replace architecture to match NATS release naming scheme
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        _ => arch,
    };
    format!("{NATS_GITHUB_RELEASE_URL}/{version}/nats-server-{version}-{os}-{arch}.tar.gz")
}

#[cfg(test)]
mod test {
    use anyhow::Result;
    use tokio::io::AsyncReadExt;

    use crate::lib::start::NatsConfig;

    #[tokio::test]
    async fn can_write_properly_formed_credsfile() -> Result<()> {
        let creds = etcetera::home_dir().unwrap().join("nats.creds");
        let config: NatsConfig = NatsConfig::new_leaf(
            "127.0.0.1",
            4243,
            None,
            "connect.ngs.global".to_string(),
            creds.clone(),
            4204,
            None,
        );

        config.write_to_path(creds.clone()).await?;

        let mut credsfile = tokio::fs::File::open(creds.clone()).await?;
        let mut contents = String::new();
        credsfile.read_to_string(&mut contents).await?;

        assert_eq!(contents, format!("\njetstream {{\n    domain={}\n    store_dir={:?}\n}}\n\nleafnodes {{\n    remotes = [\n        {{\n            url: \"{}\"\n            credentials: {:?}\n        }}\n    ]\n}}\n                \n\nwebsocket {{\n    port: 4204\n    no_tls: true\n}}\n                \n", "core", std::env::temp_dir().join("wash-jetstream-4243").display(), "connect.ngs.global", creds.to_string_lossy()));
        // A simple check to ensure we are properly escaping quotes, this is unescaped and checks for "\\"
        #[cfg(target_family = "windows")]
        assert!(creds.to_string_lossy().contains('\\'));

        Ok(())
    }
}

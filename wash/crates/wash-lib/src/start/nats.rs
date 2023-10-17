use crate::start::wait_for_server;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::fs::{metadata, write};
use tokio::process::{Child, Command};

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
/// use wash_lib::start::ensure_nats_server;
/// let res = ensure_nats_server("v2.8.4", "/tmp/").await;
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
/// use wash_lib::start::ensure_nats_server_for_os_arch_pair;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = ensure_nats_server_for_os_arch_pair(os, arch, "v2.8.4", "/tmp/").await;
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
        // NATS already exists, return early
        return Ok(nats_bin_path);
    }
    // Download NATS tarball
    download_binary_from_github(&nats_url(os, arch, version), dir, NATS_SERVER_BINARY).await
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
/// use wash_lib::start::download_nats_server;
/// let res = download_nats_server("v2.8.4", "/tmp/").await;
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
/// See the respective [NatsConfig::new_standalone] and [NatsConfig::new_leaf] implementations below for more information.
#[derive(Clone)]
pub struct NatsConfig {
    pub host: String,
    pub port: u16,
    /// The path where the NATS server will store its jetstream data. This must be different for
    /// each NATS server you spin up, otherwise they will share stream data
    pub store_dir: PathBuf,
    pub js_domain: Option<String>,
    pub remote_url: Option<String>,
    pub credentials: Option<PathBuf>,
    pub websocket_port: Option<u16>,
}

/// Returns a standalone NATS config with the following values:
/// * `host`: `127.0.0.1`
/// * `port`: `4222`
/// * `js_domain`: `Some("core")`
/// * `remote_url`: `None`
/// * `credentials`: `None`
impl Default for NatsConfig {
    fn default() -> Self {
        NatsConfig {
            host: "127.0.0.1".to_string(),
            port: 4222,
            store_dir: std::env::temp_dir().join("wash-jetstream-4222"),
            js_domain: Some("core".to_string()),
            remote_url: None,
            credentials: None,
            websocket_port: None,
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
    pub fn new_leaf(
        host: &str,
        port: u16,
        js_domain: Option<String>,
        remote_url: String,
        credentials: PathBuf,
        websocket_port: Option<u16>,
    ) -> Self {
        NatsConfig {
            host: host.to_owned(),
            port,
            store_dir: std::env::temp_dir().join(format!("wash-jetstream-{}", port)),
            js_domain,
            remote_url: Some(remote_url),
            credentials: Some(credentials),
            websocket_port,
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
            log::warn!("Listening on 0.0.0.0 is unsupported on some platforms, use 127.0.0.1 for best results")
        }
        NatsConfig {
            host: host.to_owned(),
            port,
            store_dir: std::env::temp_dir().join(format!("wash-jetstream-{}", port)),
            js_domain,
            ..Default::default()
        }
    }

    async fn write_to_path<P>(self, path: P) -> Result<()>
    where
        P: AsRef<Path>,
    {
        let leafnode_section = match (self.remote_url, self.credentials) {
            (Some(url), Some(creds)) => format!(
                r#"
leafnodes {{
    remotes = [
        {{
            url: "{}"
            credentials: {:?}
        }}
    ]
}}
                "#,
                url,
                creds.to_string_lossy()
            ),
            _ => "".to_owned(),
        };
        let websocket_section = match self.websocket_port {
            Some(port) => format!(
                r#"
websocket {{
    port: {port}
    no_tls: true
}}
                "#
            ),
            _ => "".to_owned(),
        };
        let config = format!(
            r#"
jetstream {{
    domain={}
    store_dir={:?}
}}
{leafnode_section}
{websocket_section}
"#,
            self.js_domain.unwrap_or_else(|| "core".to_string()),
            self.store_dir.as_os_str().to_string_lossy()
        );
        write(path, config).await.map_err(anyhow::Error::from)
    }
}

/// Helper function to execute a NATS server binary with required wasmCloud arguments, e.g. JetStream
/// # Arguments
///
/// * `bin_path` - Path to the nats-server binary to execute
/// * `stderr` - Specify where NATS stderr logs should be written to. If logs aren't important, use std::process::Stdio::null()
/// * `config` - Configuration for the NATS server, see [NatsConfig] for options. This config file is written alongside the nats-server binary as `nats.conf`
pub async fn start_nats_server<P, T>(bin_path: P, stderr: T, config: NatsConfig) -> Result<Child>
where
    P: AsRef<Path>,
    T: Into<Stdio>,
{
    let host_addr = format!("{}:{}", config.host, config.port);
    // If we can connect to the local port, NATS won't be able to listen on that port
    if tokio::net::TcpStream::connect(&host_addr).await.is_ok() {
        return Err(anyhow!(
            "Could not start NATS server, a process is already listening on {}:{}",
            config.host,
            config.port
        ));
    }
    let child = if let Some(parent_path) = bin_path.as_ref().parent() {
        let config_path = parent_path.join(NATS_SERVER_CONF);
        let host = config.host.to_owned();
        let port = config.port;
        config.write_to_path(&config_path).await?;
        Command::new(bin_path.as_ref())
            .stderr(stderr)
            .stdin(Stdio::null())
            .arg("-js")
            .arg("--config")
            .arg(config_path)
            .arg("--addr")
            .arg(host)
            .arg("--port")
            .arg(port.to_string())
            .arg("--pid")
            .arg(parent_path.join(NATS_SERVER_PID))
            .spawn()
            .map_err(anyhow::Error::from)
    } else {
        Err(anyhow!(
            "Could not write config to disk, couldn't find download directory"
        ))
    }?;
    wait_for_server(&host_addr, "NATS server")
        .await
        .map(|_| child)
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
    use crate::start::{
        ensure_nats_server, is_bin_installed, start_nats_server, NatsConfig, NATS_SERVER_BINARY,
    };
    use anyhow::Result;
    use std::env::temp_dir;
    use tokio::{
        fs::{create_dir_all, remove_dir_all},
        io::AsyncReadExt,
    };

    const NATS_SERVER_VERSION: &str = "v2.8.4";

    #[tokio::test]
    async fn can_handle_missing_nats_version() -> Result<()> {
        let install_dir = temp_dir().join("can_handle_missing_nats_version");
        let _ = remove_dir_all(&install_dir).await;
        create_dir_all(&install_dir).await?;
        assert!(!is_bin_installed(&install_dir, NATS_SERVER_BINARY).await);

        let res = ensure_nats_server("v300.22.1111223", &install_dir).await;
        assert!(res.is_err());

        let _ = remove_dir_all(install_dir).await;
        Ok(())
    }

    #[tokio::test]
    async fn can_download_and_start_nats() -> Result<()> {
        let install_dir = temp_dir().join("can_download_and_start_nats");
        let _ = remove_dir_all(&install_dir).await;
        create_dir_all(&install_dir).await?;
        assert!(!is_bin_installed(&install_dir, NATS_SERVER_BINARY).await);

        let res = ensure_nats_server(NATS_SERVER_VERSION, &install_dir).await;
        assert!(res.is_ok());

        let log_path = install_dir.join("nats.log");
        let log_file = tokio::fs::File::create(&log_path).await?.into_std().await;

        let config = NatsConfig::new_standalone("127.0.0.1", 10000, None);
        let child_res =
            start_nats_server(&install_dir.join(NATS_SERVER_BINARY), log_file, config).await;
        assert!(child_res.is_ok());

        // Give NATS max 5 seconds to start up
        for _ in 0..4 {
            let log_contents = tokio::fs::read_to_string(&log_path).await?;
            if log_contents.is_empty() {
                println!("NATS server hasn't started up yet, waiting 1 second");
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            } else {
                // Give just a little bit of time for the startup logs to flow in
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

                assert!(log_contents.contains("Starting nats-server"));
                assert!(log_contents.contains("Starting JetStream"));
                assert!(log_contents.contains("Server is ready"));
                break;
            }
        }

        child_res.unwrap().kill().await?;
        let _ = remove_dir_all(install_dir).await;
        Ok(())
    }

    #[tokio::test]
    async fn can_gracefully_fail_running_nats() -> Result<()> {
        let install_dir = temp_dir().join("can_gracefully_fail_running_nats");
        let _ = remove_dir_all(&install_dir).await;
        create_dir_all(&install_dir).await?;
        assert!(!is_bin_installed(&install_dir, NATS_SERVER_BINARY).await);

        let res = ensure_nats_server(NATS_SERVER_VERSION, &install_dir).await;
        assert!(res.is_ok());

        let config = NatsConfig::new_standalone("127.0.0.1", 10003, Some("extender".to_string()));
        let nats_one = start_nats_server(
            &install_dir.join(NATS_SERVER_BINARY),
            std::process::Stdio::null(),
            config.clone(),
        )
        .await;
        assert!(nats_one.is_ok());

        // Give NATS a few seconds to start up and listen
        tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
        let log_path = install_dir.join("nats.log");
        let log = std::fs::File::create(&log_path)?;
        let nats_two = start_nats_server(&install_dir.join(NATS_SERVER_BINARY), log, config).await;
        assert!(nats_two.is_err());

        nats_one.unwrap().kill().await?;
        let _ = remove_dir_all(install_dir).await;

        Ok(())
    }

    #[tokio::test]
    async fn can_write_properly_formed_credsfile() -> Result<()> {
        let install_dir = temp_dir().join("can_write_properly_formed_credsfile");
        let _ = remove_dir_all(&install_dir).await;
        create_dir_all(&install_dir).await?;
        assert!(
            !is_bin_installed(&install_dir, NATS_SERVER_BINARY).await,
            "NATS should not be installed"
        );

        let res = ensure_nats_server(NATS_SERVER_VERSION, &install_dir).await;
        assert!(res.is_ok(), "NATS should be able to start");

        let creds = dirs::home_dir().unwrap().join("nats.creds");
        let config: NatsConfig = NatsConfig::new_leaf(
            "127.0.0.1",
            4243,
            None,
            "connect.ngs.global".to_string(),
            creds.clone(),
            Some(4204),
        );

        config.write_to_path(creds.clone()).await?;

        let mut credsfile = tokio::fs::File::open(creds.clone()).await?;
        let mut contents = String::new();
        credsfile.read_to_string(&mut contents).await?;

        assert_eq!(contents, format!("\njetstream {{\n    domain={}\n    store_dir={:?}\n}}\n\nleafnodes {{\n    remotes = [\n        {{\n            url: \"{}\"\n            credentials: {:?}\n        }}\n    ]\n}}\n                \n\nwebsocket {{\n    port: 4204\n    no_tls: true\n}}\n                \n", "core", std::env::temp_dir().join("wash-jetstream-4243").display(), "connect.ngs.global", creds.to_string_lossy()));
        // A simple check to ensure we are properly escaping quotes, this is unescaped and checks for "\\"
        #[cfg(target_family = "windows")]
        assert!(creds.to_string_lossy().contains("\\"));

        let _ = remove_dir_all(install_dir).await;
        Ok(())
    }
}

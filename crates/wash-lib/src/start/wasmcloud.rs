use std::collections::HashMap;

use anyhow::{bail, Result};
use reqwest::StatusCode;
use tokio::fs::{create_dir_all, metadata, File};
use tokio::process::{Child, Command};
use tokio_stream::StreamExt;
use tokio_util::io::StreamReader;
use tracing::warn;

#[cfg(target_family = "unix")]
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[cfg(target_family = "unix")]
use command_group::AsyncCommandGroup;

use super::get_download_client;

const WASMCLOUD_GITHUB_RELEASE_URL: &str =
    "https://github.com/wasmCloud/wasmCloud/releases/download";
#[cfg(target_family = "unix")]
pub const WASMCLOUD_HOST_BIN: &str = "wasmcloud_host";
#[cfg(target_family = "windows")]
pub const WASMCLOUD_HOST_BIN: &str = "wasmcloud_host.exe";

// Any version of wasmCloud under 0.81 does not support wasmtime 16 wit worlds and is incompatible.
const MINIMUM_WASMCLOUD_VERSION: &str = "0.81.0";

/// A wrapper around the [`ensure_wasmcloud_for_os_arch_pair`] function that uses the
/// architecture and operating system of the current host machine.
///
/// # Arguments
///
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`. Must be at least v0.63.0.
/// * `dir` - Where to unpack the wasmCloud host contents into
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::ensure_wasmcloud;
/// let res = ensure_wasmcloud("v0.63.0", "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/v0.63.0/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn ensure_wasmcloud<P>(version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    ensure_wasmcloud_for_os_arch_pair(version, dir).await
}

/// Ensures the `wasmcloud_host` application is installed, returning the path to the executable
/// early if it exists or downloading the specified GitHub release version of the wasmCloud host
/// from <https://github.com/wasmCloud/wasmcloud-otp/releases/> and unpacking the contents for a
/// specified OS/ARCH pair to a directory. Returns the path to the executable.
///
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`. Must be
///   at least v0.63.0.
/// * `dir` - Where to unpack the wasmCloud host contents into. This should be the root level
///   directory where to store hosts. Each host will be stored in a directory maching its version
///   (e.g. "/tmp/wasmcloud/v0.63.0")
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::ensure_wasmcloud_for_os_arch_pair;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = ensure_wasmcloud_for_os_arch_pair("v0.63.0", "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/v0.63.0/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn ensure_wasmcloud_for_os_arch_pair<P>(version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    check_version(version)?;
    if let Some(dir) = find_wasmcloud_binary(&dir, version).await {
        // wasmCloud already exists, return early
        return Ok(dir);
    }
    // Download wasmCloud host tarball
    download_wasmcloud_for_os_arch_pair(version, dir).await
}

/// A wrapper around the [`download_wasmcloud_for_os_arch_pair`] function that uses the
/// architecture and operating system of the current host machine.
///
/// # Arguments
///
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to unpack the wasmCloud host contents into. This should be the root level
///   directory where to store hosts. Each host will be stored in a directory maching its version
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::download_wasmcloud;
/// let res = download_wasmcloud("v0.57.1", "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/v0.63.0/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn download_wasmcloud<P>(version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    download_wasmcloud_for_os_arch_pair(version, dir).await
}

/// Downloads the specified GitHub release version of the wasmCloud host from
/// <https://github.com/wasmCloud/wasmcloud-otp/releases/> and unpacking the contents for a
/// specified OS/ARCH pair to a directory. Returns the path to the Elixir executable.
///
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to unpack the wasmCloud host contents into. This should be the root level
///   directory where to store hosts. Each host will be stored in a directory maching its version
/// # Examples
///
/// ```rust,ignore
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::download_wasmcloud_for_os_arch_pair;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = download_wasmcloud_for_os_arch_pair("v0.63.0", "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/v0.63.0/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn download_wasmcloud_for_os_arch_pair<P>(version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let url = wasmcloud_url(version);
    // NOTE(brooksmtownsend): This seems like a lot of work when I really just want to use AsyncRead
    // to pipe the response body into a file. I'm not sure if there's a better way to do this.
    let download_response = get_download_client()?.get(&url).send().await?;
    if download_response.status() != StatusCode::OK {
        bail!(
            "failed to download wasmCloud host from {}. Status code: {}",
            url,
            download_response.status()
        );
    }

    let burrito_bites_stream = download_response
        .bytes_stream()
        .map(|result| result.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)));
    let mut wasmcloud_host_burrito = StreamReader::new(burrito_bites_stream);
    let version_dir = dir.as_ref().join(version);
    let file_path = version_dir.join(WASMCLOUD_HOST_BIN);
    if let Some(parent_folder) = file_path.parent() {
        // If the user doesn't have permission to create files in the provided directory,
        // this will bubble the error up noting permission denied
        create_dir_all(parent_folder).await?;
    }
    if let Ok(mut wasmcloud_file) = File::create(&file_path).await {
        // This isn't an `if let` to avoid a Windows lint warning
        if file_path.file_name().is_some() {
            // Set permissions of executable files and binaries to allow executing
            #[cfg(target_family = "unix")]
            {
                let mut perms = wasmcloud_file.metadata().await?.permissions();
                perms.set_mode(0o755);
                wasmcloud_file.set_permissions(perms).await?;
            }
        }
        tokio::io::copy(&mut wasmcloud_host_burrito, &mut wasmcloud_file).await?;
    }

    // Return success if wasmCloud components exist, error otherwise
    match find_wasmcloud_binary(&dir, version).await {
        Some(path) => Ok(path),
        None => bail!("wasmCloud was not installed successfully, please see logs"),
    }
}

/// Helper function to start a wasmCloud host given the path to the burrito release application
/// # Arguments
///
/// * `bin_path` - Path to the `wasmcloud_host` burrito application
/// * `stdout` - Specify where wasmCloud stdout logs should be written to. Logs can be written to stdout by the erlang process
/// * `stderr` - Specify where wasmCloud stderr logs should be written to. Logs are written to stderr that are generated by wasmCloud
/// * `env_vars` - Environment variables to pass to the host, see <https://wasmcloud.dev/reference/host-runtime/host_configure/#supported-configuration-variables> for details
pub async fn start_wasmcloud_host<P, T, S>(
    bin_path: P,
    stdout: T,
    stderr: S,
    env_vars: HashMap<String, String>,
) -> Result<Child>
where
    P: AsRef<Path>,
    T: Into<Stdio>,
    S: Into<Stdio>,
{
    // Constructing this object in one step results in a temporary value that's dropped
    let mut cmd = Command::new(bin_path.as_ref());
    let cmd = cmd
        // wasmCloud host logs are sent to stderr as of https://github.com/wasmCloud/wasmcloud-otp/pull/418
        .stderr(stderr)
        .stdout(stdout)
        // NOTE: while normally we might want to kill_on_drop here, the tests that use this function
        // manually manage the process that is spawned (see can_download_and_start_wasmcloud)
        .stdin(Stdio::null())
        .envs(&env_vars);

    #[cfg(target_family = "unix")]
    {
        Ok(cmd.group_spawn()?.into_inner())
    }
    #[cfg(target_family = "windows")]
    {
        Ok(cmd.spawn()?)
    }
}

/// Helper function to indicate if the wasmCloud host tarball is successfully
/// installed in a directory. Returns the path to the binary if it exists
pub async fn find_wasmcloud_binary<P>(dir: P, version: &str) -> Option<PathBuf>
where
    P: AsRef<Path>,
{
    let versioned_dir = dir.as_ref().join(version);
    let bin_file = versioned_dir.join(WASMCLOUD_HOST_BIN);

    metadata(&bin_file).await.is_ok().then_some(bin_file)
}

/// Helper function to determine the wasmCloud host release path given an os/arch and version
fn wasmcloud_url(version: &str) -> String {
    #[cfg(target_os = "android")]
    let os = "linux-android";

    #[cfg(target_os = "macos")]
    let os = "apple-darwin";

    #[cfg(all(target_os = "linux", not(target_arch = "riscv64")))]
    let os = "unknown-linux-musl";

    #[cfg(all(target_os = "linux", target_arch = "riscv64"))]
    let os = "unknown-linux-gnu";

    #[cfg(target_os = "windows")]
    let os = "pc-windows-msvc.exe";
    format!(
        "{WASMCLOUD_GITHUB_RELEASE_URL}/{version}/wasmcloud-{arch}-{os}",
        arch = std::env::consts::ARCH
    )
}

/// Helper function to ensure the version of wasmCloud is above the minimum
/// supported version (v0.63.0) that runs burrito releases
fn check_version(version: &str) -> Result<()> {
    let version_req = semver::VersionReq::parse(&format!(">={MINIMUM_WASMCLOUD_VERSION}"))?;
    match semver::Version::parse(version.trim_start_matches('v')) {
        Ok(parsed_version) if !parsed_version.pre.is_empty() => {
            warn!("Using prerelease version {} of wasmCloud", version);
            Ok(())
        }
        Ok(parsed_version) if !version_req.matches(&parsed_version) => bail!(
            "wasmCloud version {} is earlier than the minimum supported version of v{}",
            version,
            MINIMUM_WASMCLOUD_VERSION
        ),
        Ok(_ver) => Ok(()),
        Err(_parse_err) => {
            warn!("Failed to parse wasmCloud version as a semantic version, download may fail");
            Ok(())
        }
    }
}

#[cfg(test)]
mod test {
    use super::{check_version, ensure_wasmcloud, wasmcloud_url, MINIMUM_WASMCLOUD_VERSION};
    use crate::common::CommandGroupUsage;
    use crate::start::{
        ensure_nats_server, ensure_wasmcloud_for_os_arch_pair, find_wasmcloud_binary,
        is_bin_installed, start_nats_server, start_wasmcloud_host, NatsConfig, NATS_SERVER_BINARY,
    };

    use anyhow::{Context, Result};
    use reqwest::StatusCode;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::{collections::HashMap, env::temp_dir};
    use tokio::fs::{create_dir_all, remove_dir_all};
    use tokio::net::TcpListener;
    use tokio::time::Duration;

    const WASMCLOUD_VERSION: &str = "v0.81.0";

    /// Returns an open port on the interface, searching within the range endpoints, inclusive
    async fn find_open_port() -> Result<u16> {
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .await
            .context("failed to bind random port")?
            .local_addr()
            .map(|addr| addr.port())
            .context("failed to get local address from opened TCP socket")
    }

    #[tokio::test]
    #[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
    async fn can_request_supported_wasmcloud_urls() {
        assert_eq!(
            reqwest::get(wasmcloud_url(WASMCLOUD_VERSION))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    #[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
    async fn can_download_wasmcloud_host() {
        let download_dir = temp_dir().join("can_download_wasmcloud_host");
        let res = ensure_wasmcloud_for_os_arch_pair(WASMCLOUD_VERSION, &download_dir)
            .await
            .expect("Should be able to download tarball");

        // Make sure we can find the binary and that it matches the path we got back from ensure
        assert_eq!(
            find_wasmcloud_binary(&download_dir, WASMCLOUD_VERSION)
                .await
                .expect("Should have found installed wasmcloud"),
            res
        );

        let _ = remove_dir_all(download_dir).await;
    }

    #[tokio::test]
    #[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
    async fn can_handle_missing_wasmcloud_version() {
        let download_dir = temp_dir().join("can_handle_missing_wasmcloud_version");
        let res = ensure_wasmcloud("v10233.123.3.4", &download_dir).await;

        assert!(res.is_err());
        let _ = remove_dir_all(download_dir).await;
    }

    #[tokio::test]
    #[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
    async fn can_download_different_versions() {
        let download_dir = temp_dir().join("can_download_different_versions");
        ensure_wasmcloud_for_os_arch_pair(WASMCLOUD_VERSION, &download_dir)
            .await
            .expect("Should be able to download host");

        assert!(
            find_wasmcloud_binary(&download_dir, WASMCLOUD_VERSION)
                .await
                .is_some(),
            "wasmCloud should be installed"
        );

        // Just to triple check, make sure the paths actually exist
        assert!(
            download_dir.join(WASMCLOUD_VERSION).exists(),
            "Directory should exist"
        );

        let _ = remove_dir_all(download_dir).await;
    }

    const NATS_SERVER_VERSION: &str = "v2.10.7";

    #[tokio::test]
    #[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
    async fn can_download_and_start_wasmcloud() -> anyhow::Result<()> {
        #[cfg(target_family = "unix")]
        let install_dir = temp_dir().join("can_download_and_start_wasmcloud");
        // This is a very specific hack to download wasmCloud to the same _drive_ on Windows
        // Turns out the mix release .bat file can't support executing an application that's installed
        // on a different drive (e.g. running wasmCloud on the D: drive from the C: drive), which is what
        // GitHub Actions does by default (runs in the D: drive, creates temp dir in the C: drive)
        #[cfg(target_family = "windows")]
        let install_dir = std::env::current_dir()?.join("can_download_and_start_wasmcloud");
        let _ = remove_dir_all(&install_dir).await;
        create_dir_all(&install_dir).await?;
        assert!(find_wasmcloud_binary(&install_dir, WASMCLOUD_VERSION)
            .await
            .is_none());

        // Install and start NATS server for this test
        let nats_port = find_open_port().await?;
        let nats_ws_port = find_open_port().await?;
        assert!(ensure_nats_server(NATS_SERVER_VERSION, &install_dir)
            .await
            .is_ok());
        assert!(is_bin_installed(&install_dir, NATS_SERVER_BINARY).await);
        let mut config = NatsConfig::new_standalone("127.0.0.1", nats_port, None);
        config.websocket_port = nats_ws_port;
        let mut nats_child = start_nats_server(
            install_dir.join(NATS_SERVER_BINARY),
            std::process::Stdio::null(),
            config,
            CommandGroupUsage::UseParent,
        )
        .await
        .expect("Unable to start nats process");

        let wasmcloud_binary = ensure_wasmcloud(WASMCLOUD_VERSION, &install_dir)
            .await
            .expect("Unable to ensure wasmcloud");

        let stderr_log_path = wasmcloud_binary
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("wasmcloud_stderr.log");
        let stderr_log_file = tokio::fs::File::create(&stderr_log_path)
            .await?
            .into_std()
            .await;
        let stdout_log_path = wasmcloud_binary
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("wasmcloud_stdout.log");
        let stdout_log_file = tokio::fs::File::create(&stdout_log_path)
            .await?
            .into_std()
            .await;

        let mut host_env = HashMap::new();
        host_env.insert("WASMCLOUD_RPC_PORT".to_string(), nats_port.to_string());
        host_env.insert("WASMCLOUD_CTL_PORT".to_string(), nats_port.to_string());
        let mut host_child = start_wasmcloud_host(
            &wasmcloud_binary,
            stdout_log_file,
            stderr_log_file,
            host_env,
        )
        .await
        .expect("Unable to start wasmcloud host");

        // Wait at most 10 seconds for wasmcloud to start
        println!("waiting for wasmcloud to start..");
        let startup_log_path = stderr_log_path.clone();
        tokio::time::timeout(Duration::from_secs(10), async move {
            loop {
                match tokio::fs::read_to_string(&startup_log_path).await {
                    Ok(file_contents) if !file_contents.is_empty() => break,
                    _ => {
                        println!("wasmCloud hasn't started up yet, waiting 1 second");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        })
        .await
        .context("failed to start wasmcloud (log path is missing)")?;

        // Wait for up to 15 seconds for the logs to contain expected lines
        println!("wasmCloud has started, waiting for expected startup logs...");
        let startup_log_path = stderr_log_path.clone();
        tokio::time::timeout(Duration::from_secs(15), async move {
            loop {
                match tokio::fs::read_to_string(&startup_log_path).await {
                    Ok(file_contents) => {
                        if file_contents.contains("wasmCloud host started") {
                            // After wasmcloud says it's ready, it still requires some seconds to start up.
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            break;
                        }
                    }
                    _ => {
                        println!("no host startup logs in output yet, waiting 1 second");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        })
        .await
        .context("failed to start wasmcloud (logs did not contain expected content)")?;

        // We support multiple hosts, so this should work fine
        let mut host_env = HashMap::new();
        host_env.insert("WASMCLOUD_RPC_PORT".to_string(), nats_port.to_string());
        host_env.insert("WASMCLOUD_CTL_PORT".to_string(), nats_port.to_string());
        let child_res = start_wasmcloud_host(
            &wasmcloud_binary,
            std::process::Stdio::null(),
            std::process::Stdio::null(),
            host_env,
        )
        .await;
        assert!(child_res.is_ok());
        child_res.unwrap().kill().await?;

        host_child.kill().await?;
        nats_child.kill().await?;
        let _ = remove_dir_all(install_dir).await;
        Ok(())
    }

    #[tokio::test]
    async fn can_properly_deny_too_old_hosts() -> anyhow::Result<()> {
        // Ensure we allow versions >= 0.81.0
        assert!(check_version("v0.81.0").is_ok());
        assert!(check_version(MINIMUM_WASMCLOUD_VERSION).is_ok());

        // Ensure we allow prerelease tags for testing
        assert!(check_version("v0.81.0-rc1").is_ok());

        // Ensure we deny versions < MINIMUM_WASMCLOUD_VERSION
        assert!(check_version("v0.80.99").is_err());

        if let Err(e) = check_version("v0.56.0") {
            assert_eq!(e.to_string(), format!("wasmCloud version v0.56.0 is earlier than the minimum supported version of v{MINIMUM_WASMCLOUD_VERSION}"));
        } else {
            panic!("v0.56.0 should be before the minimum version")
        }

        // The check_version will allow bad semantic versions, rather than failing immediately
        assert!(check_version("ungabunga").is_ok());
        assert!(check_version("v11.1").is_ok());

        Ok(())
    }
}

use anyhow::{anyhow, Result};
use async_compression::tokio::bufread::GzipDecoder;
#[cfg(target_family = "unix")]
use command_group::AsyncCommandGroup;
use futures::future::join_all;
use std::collections::HashMap;
use std::io::Cursor;
#[cfg(target_family = "unix")]
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::fs::{create_dir_all, metadata, File};
use tokio::process::{Child, Command};
use tokio_stream::StreamExt;
use tokio_tar::Archive;

const WASMCLOUD_GITHUB_RELEASE_URL: &str =
    "https://github.com/wasmCloud/wasmcloud-otp/releases/download";
#[cfg(target_family = "unix")]
pub(crate) const WASMCLOUD_HOST_BIN: &str = "bin/wasmcloud_host";
#[cfg(target_family = "windows")]
pub(crate) const WASMCLOUD_HOST_BIN: &str = "bin\\wasmcloud_host.bat";

// Any version of wasmCloud under 0.57.0 uses distillery releases and is incompatible
const MINIMUM_WASMCLOUD_VERSION: &str = "0.57.0";

/// A wrapper around the [ensure_wasmcloud_for_os_arch_pair] function that uses the
/// architecture and operating system of the current host machine.
///
/// # Arguments
///
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`. Must be at least v0.57.0.
/// * `dir` - Where to unpack the wasmCloud host contents into
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::ensure_wasmcloud;
/// let res = ensure_wasmcloud("v0.57.1", "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/bin/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn ensure_wasmcloud<P>(version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    ensure_wasmcloud_for_os_arch_pair(std::env::consts::OS, std::env::consts::ARCH, version, dir)
        .await
}

/// Ensures the `wasmcloud_host` application is installed, returning the path to the executable early if it exists or
/// downloading the specified GitHub release version of the wasmCloud host from <https://github.com/wasmCloud/wasmcloud-otp/releases/>
/// and unpacking the contents for a specified OS/ARCH pair to a directory. Returns the path to the Elixir executable.
///
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`. Must be at least v0.57.0.
/// * `dir` - Where to unpack the wasmCloud host contents into
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::ensure_wasmcloud_for_os_arch_pair;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = ensure_wasmcloud_for_os_arch_pair(os, arch, "v0.57.1", "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/bin/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn ensure_wasmcloud_for_os_arch_pair<P>(
    os: &str,
    arch: &str,
    version: &str,
    dir: P,
) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    check_version(version)?;
    if is_wasmcloud_installed(&dir).await {
        // wasmCloud already exists, return early
        return Ok(dir.as_ref().join(WASMCLOUD_HOST_BIN));
    }
    // Download wasmCloud host tarball
    download_wasmcloud_for_os_arch_pair(os, arch, version, dir).await
}

/// A wrapper around the [download_wasmcloud_for_os_arch_pair] function that uses the
/// architecture and operating system of the current host machine.
///
/// # Arguments
///
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to unpack the wasmCloud host contents into
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::download_wasmcloud;
/// let res = download_wasmcloud("v0.57.1", "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/bin/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn download_wasmcloud<P>(version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    download_wasmcloud_for_os_arch_pair(std::env::consts::OS, std::env::consts::ARCH, version, dir)
        .await
}

/// Downloads the specified GitHub release version of the wasmCloud host from <https://github.com/wasmCloud/wasmcloud-otp/releases/>
/// and unpacking the contents for a specified OS/ARCH pair to a directory. Returns the path to the Elixir executable.
///
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to unpack the wasmCloud host contents into
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::download_wasmcloud_for_os_arch_pair;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = download_wasmcloud_for_os_arch_pair(os, arch, "v0.57.1", "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/bin/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn download_wasmcloud_for_os_arch_pair<P>(
    os: &str,
    arch: &str,
    version: &str,
    dir: P,
) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let url = wasmcloud_url(os, arch, version);
    let body = reqwest::get(url).await?.bytes().await?;
    let cursor = Cursor::new(body);
    let mut wasmcloud_host = Archive::new(Box::new(GzipDecoder::new(cursor)));
    let mut entries = wasmcloud_host.entries()?;
    // Copy all of the files out of the tarball into the bin directory
    let mut executable_path = None;
    while let Some(res) = entries.next().await {
        let mut entry = res.map_err(|_e| {
            anyhow!(
                "Failed to retrieve file from archive, ensure wasmcloud_host version '{}' exists",
                version
            )
        })?;
        if let Ok(path) = entry.path() {
            let file_path = dir.as_ref().join(path);
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
                        let file_name = file_path.file_name().unwrap().to_string_lossy();
                        if file_path.to_string_lossy().contains("bin")
                            || file_name.contains(".sh")
                            || file_name.contains(".bat")
                            || file_name.eq("iex")
                            || file_name.eq("elixir")
                            || file_name.eq("wasmcloud_host")
                        {
                            let mut perms = wasmcloud_file.metadata().await?.permissions();
                            perms.set_mode(0o755);
                            wasmcloud_file.set_permissions(perms).await?;
                        }
                    }

                    // Set the executable path for return
                    if file_path.ends_with(WASMCLOUD_HOST_BIN) {
                        executable_path = Some(file_path.clone())
                    }
                }
                tokio::io::copy(&mut entry, &mut wasmcloud_file).await?;
            }
        }
    }

    // Return success if wasmCloud components exist, error otherwise
    match (is_wasmcloud_installed(&dir).await, executable_path) {
        (true, Some(path)) => Ok(path),
        (true, None) => Err(anyhow!(
            "wasmCloud was installed but the binary could not be located"
        )),
        (false, _) => Err(anyhow!(
            "wasmCloud was not installed successfully, please see logs"
        )),
    }
}
/// Helper function to start a wasmCloud host given the path to the elixir release script
/// /// # Arguments
///
/// * `bin_path` - Path to the wasmcloud_host script to execute
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
    // If we can connect to the local port, a wasmCloud host won't be able to listen on that port
    let port = env_vars
        .get("PORT")
        .cloned()
        .unwrap_or_else(|| "4000".to_string());
    if tokio::net::TcpStream::connect(format!("localhost:{}", port))
        .await
        .is_ok()
    {
        return Err(anyhow!(
            "Could not start wasmCloud, a process is already listening on localhost:{}",
            port
        ));
    }

    #[cfg(target_family = "unix")]
    if let Ok(output) = Command::new(bin_path.as_ref()).arg("pid").output().await {
        // Stderr will include :nodedown if no other host is running, otherwise
        // stdout will contain the PID
        if !String::from_utf8_lossy(&output.stderr).contains(":nodedown") {
            return Err(anyhow!(
                "Another wasmCloud host is already running on this machine with PID {}",
                String::from_utf8_lossy(&output.stdout)
            ));
        }
    }

    // Constructing this object in one step results in a temporary value that's dropped
    let mut cmd = Command::new(bin_path.as_ref());
    let cmd = cmd
        // wasmCloud host logs are sent to stderr as of https://github.com/wasmCloud/wasmcloud-otp/pull/418
        .stderr(stderr)
        .stdout(stdout)
        .stdin(Stdio::null())
        .envs(&env_vars)
        .arg("start");

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
/// installed in a directory
pub async fn is_wasmcloud_installed<P>(dir: P) -> bool
where
    P: AsRef<Path>,
{
    let bin_dir = dir.as_ref().join("bin");
    let release_script = dir.as_ref().join(WASMCLOUD_HOST_BIN);
    let lib_dir = dir.as_ref().join("lib");
    let releases_dir = dir.as_ref().join("releases");
    let file_checks = vec![
        metadata(dir.as_ref()),
        metadata(&bin_dir),
        metadata(&release_script),
        metadata(&lib_dir),
        metadata(&releases_dir),
    ];
    join_all(file_checks).await.iter().all(|i| i.is_ok())
}

/// Helper function to determine the wasmCloud host release path given an os/arch and version
fn wasmcloud_url(os: &str, arch: &str, version: &str) -> String {
    format!(
        "{}/{}/{}-{}.tar.gz",
        WASMCLOUD_GITHUB_RELEASE_URL, version, arch, os
    )
}

/// Helper function to ensure the version of wasmCloud is above the minimum
/// supported version (v0.57.0) that runs mix releases
fn check_version(version: &str) -> Result<()> {
    let version_req = semver::VersionReq::parse(&format!(">={}", MINIMUM_WASMCLOUD_VERSION))?;
    match semver::Version::parse(version.trim_start_matches('v')) {
        Ok(parsed_version) if !version_req.matches(&parsed_version) => Err(anyhow!(
            "wasmCloud version {} is earlier than the minimum supported version of v{}",
            version,
            MINIMUM_WASMCLOUD_VERSION
        )),
        Ok(_ver) => Ok(()),
        Err(_parse_err) => {
            log::warn!(
                "Failed to parse wasmCloud version as a semantic version, download may fail"
            );
            Ok(())
        }
    }
}
#[cfg(test)]
mod test {
    use super::{check_version, ensure_wasmcloud, wasmcloud_url};
    use crate::start::{
        ensure_nats_server, ensure_wasmcloud_for_os_arch_pair, is_nats_installed,
        is_wasmcloud_installed, start_nats_server, start_wasmcloud_host, NatsConfig,
        NATS_SERVER_BINARY,
    };
    use reqwest::StatusCode;
    use std::{collections::HashMap, env::temp_dir};
    use tokio::fs::{create_dir_all, remove_dir_all};
    const WASMCLOUD_VERSION: &str = "v0.57.1";

    #[tokio::test]
    async fn can_request_supported_wasmcloud_urls() {
        let host_tarballs = vec![
            wasmcloud_url("linux", "aarch64", WASMCLOUD_VERSION),
            wasmcloud_url("linux", "x86_64", WASMCLOUD_VERSION),
            wasmcloud_url("macos", "aarch64", WASMCLOUD_VERSION),
            wasmcloud_url("macos", "x86_64", WASMCLOUD_VERSION),
            wasmcloud_url("windows", "x86_64", WASMCLOUD_VERSION),
        ];
        for tarball_url in host_tarballs {
            assert_eq!(
                reqwest::get(tarball_url).await.unwrap().status(),
                StatusCode::OK
            );
        }
    }

    #[tokio::test]
    async fn can_download_wasmcloud_tarball() {
        let download_dir = temp_dir().join("can_download_wasmcloud_tarball");
        let res =
            ensure_wasmcloud_for_os_arch_pair("macos", "aarch64", WASMCLOUD_VERSION, &download_dir)
                .await;
        assert!(res.is_ok());
        assert!(is_wasmcloud_installed(&download_dir).await);
        let _ = remove_dir_all(download_dir).await;
    }

    #[tokio::test]
    async fn can_handle_missing_wasmcloud_version() {
        let download_dir = temp_dir().join("can_handle_missing_wasmcloud_version");
        let res = ensure_wasmcloud("v010233.123.3.4", &download_dir).await;

        assert!(res.is_err());
        let _ = remove_dir_all(download_dir).await;
    }

    const NATS_SERVER_VERSION: &str = "v2.8.4";
    const WASMCLOUD_HOST_VERSION: &str = "v0.57.1";

    #[tokio::test]
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
        assert!(!is_wasmcloud_installed(&install_dir).await);

        // Install and start NATS server for this test
        let nats_port = 10004;
        assert!(ensure_nats_server(NATS_SERVER_VERSION, &install_dir)
            .await
            .is_ok());
        assert!(is_nats_installed(&install_dir).await);
        let config = NatsConfig::new_standalone("127.0.0.1", nats_port, None);
        let nats_child = start_nats_server(
            install_dir.join(NATS_SERVER_BINARY),
            std::process::Stdio::null(),
            config,
        )
        .await;
        assert!(nats_child.is_ok());

        let res = ensure_wasmcloud(WASMCLOUD_HOST_VERSION, &install_dir).await;
        assert!(res.is_ok());

        let stderr_log_path = install_dir.join("wasmcloud_stderr.log");
        let stderr_log_file = tokio::fs::File::create(&stderr_log_path)
            .await?
            .into_std()
            .await;
        let stdout_log_path = install_dir.join("wasmcloud_stdout.log");
        let stdout_log_file = tokio::fs::File::create(&stdout_log_path)
            .await?
            .into_std()
            .await;

        let mut host_env = HashMap::new();
        host_env.insert("WASMCLOUD_RPC_PORT".to_string(), nats_port.to_string());
        host_env.insert("WASMCLOUD_CTL_PORT".to_string(), nats_port.to_string());
        host_env.insert("WASMCLOUD_PROV_RPC_PORT".to_string(), nats_port.to_string());
        let host_child = start_wasmcloud_host(
            &install_dir.join(crate::start::wasmcloud::WASMCLOUD_HOST_BIN),
            stdout_log_file,
            stderr_log_file,
            host_env,
        )
        .await;
        assert!(host_child.is_ok());

        // Give wasmCloud max 15 seconds to start up
        for _ in 0..14 {
            let log_contents = tokio::fs::read_to_string(&stderr_log_path).await?;
            if log_contents.is_empty() {
                println!("wasmCloud hasn't started up yet, waiting 1 second");
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            } else {
                // Give just a little bit of time for the startup logs to flow in, re-read logs
                tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
                let log_contents = tokio::fs::read_to_string(&stderr_log_path).await?;
                assert!(log_contents
                    .contains("Connecting to control interface NATS without authentication"));
                assert!(
                    log_contents.contains("Connecting to lattice rpc NATS without authentication")
                );
                assert!(log_contents.contains("Started wasmCloud OTP Host Runtime"));
                break;
            }
        }

        // Should fail because the port is already in use by another host
        let mut host_env = HashMap::new();
        host_env.insert("WASMCLOUD_RPC_PORT".to_string(), nats_port.to_string());
        host_env.insert("WASMCLOUD_CTL_PORT".to_string(), nats_port.to_string());
        host_env.insert("WASMCLOUD_PROV_RPC_PORT".to_string(), nats_port.to_string());
        let child_res = start_wasmcloud_host(
            &install_dir.join(crate::start::wasmcloud::WASMCLOUD_HOST_BIN),
            std::process::Stdio::null(),
            std::process::Stdio::null(),
            host_env,
        )
        .await;
        assert!(child_res.is_err());

        // Should fail because another erlang wasmcloud_host node is running
        #[cfg(target_family = "unix")]
        // Windows is unable to properly check running erlang nodes with `pid`
        {
            let mut host_env = HashMap::new();
            host_env.insert("PORT".to_string(), "4002".to_string());
            host_env.insert("WASMCLOUD_RPC_PORT".to_string(), nats_port.to_string());
            host_env.insert("WASMCLOUD_CTL_PORT".to_string(), nats_port.to_string());
            host_env.insert("WASMCLOUD_PROV_RPC_PORT".to_string(), nats_port.to_string());
            let child_res = start_wasmcloud_host(
                &install_dir.join(crate::start::wasmcloud::WASMCLOUD_HOST_BIN),
                std::process::Stdio::null(),
                std::process::Stdio::null(),
                host_env,
            )
            .await;
            assert!(child_res.is_err());
        }

        host_child.unwrap().kill().await?;
        nats_child.unwrap().kill().await?;
        let _ = remove_dir_all(install_dir).await;
        Ok(())
    }

    #[tokio::test]
    async fn can_properly_deny_distillery_release_hosts() -> anyhow::Result<()> {
        // Ensure we allow versions >= 0.57.0
        assert!(check_version("v1.56.0").is_ok());
        assert!(check_version("v0.57.0").is_ok());
        assert!(check_version("v0.57.1").is_ok());
        assert!(check_version("v0.57.2").is_ok());
        assert!(check_version("v0.58.0").is_ok());
        assert!(check_version("v0.100.0").is_ok());
        assert!(check_version("v0.203.0").is_ok());

        // Ensure we deny versions < 0.57.0
        assert!(check_version("v0.48.0").is_err());
        assert!(check_version("v0.56.0").is_err());
        assert!(check_version("v0.12.0").is_err());
        assert!(check_version("v0.56.999").is_err());
        if let Err(e) = check_version("v0.56.0") {
            assert_eq!(e.to_string(), "wasmCloud version v0.56.0 is earlier than the minimum supported version of v0.57.0");
        } else {
            panic!("v0.56.0 should be before the minimum version")
        }

        // The check_version will allow bad semantic versions, rather than failing immediately
        assert!(check_version("ungabunga").is_ok());
        assert!(check_version("v11.1").is_ok());

        Ok(())
    }
}

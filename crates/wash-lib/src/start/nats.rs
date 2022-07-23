use anyhow::{anyhow, Result};
use async_compression::tokio::bufread::GzipDecoder;
#[cfg(target_family = "unix")]
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::{ffi::OsStr, io::Cursor};
use tokio::fs::{create_dir_all, metadata, File};
use tokio::process::{Child, Command};
use tokio_stream::StreamExt;
use tokio_tar::Archive;

const NATS_GITHUB_RELEASE_URL: &str = "https://github.com/nats-io/nats-server/releases/download";
#[cfg(target_family = "unix")]
pub(crate) const NATS_SERVER_BINARY: &str = "nats-server";
#[cfg(target_family = "windows")]
pub(crate) const NATS_SERVER_BINARY: &str = "nats-server.exe";

/// A wrapper around the [ensure_nats_server_for_os_arch_pair] function that uses the
/// architecture and operating system of the current host.
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
    let url = nats_url(os, arch, version);
    let body = match reqwest::get(url).await {
        Ok(resp) => resp.bytes().await?,
        Err(e) => return Err(anyhow!("Failed to request NATS release: {:?}", e)),
    };
    let cursor = Cursor::new(body);
    let mut nats_server = Archive::new(Box::new(GzipDecoder::new(cursor)));

    // Look for nats-server binary and only extract that
    let mut entries = nats_server.entries()?;
    while let Some(res) = entries.next().await {
        let mut entry = res.map_err(|_e| {
            anyhow!(
                "Failed to retrieve file from archive, ensure NATS server {} exists",
                version
            )
        })?;
        if let Ok(tar_path) = entry.path() {
            match tar_path.file_name() {
                Some(name) if name == OsStr::new(NATS_SERVER_BINARY) => {
                    // Ensure target directory exists
                    create_dir_all(&dir).await?;
                    let mut nats_server = File::create(&nats_bin_path).await?;
                    // Make nats-server executable
                    #[cfg(target_family = "unix")]
                    {
                        let mut permissions = nats_server.metadata().await?.permissions();
                        // Read/write/execute for owner and read/execute for others. This is what `cargo install` does
                        permissions.set_mode(0o755);
                        nats_server.set_permissions(permissions).await?;
                    }

                    tokio::io::copy(&mut entry, &mut nats_server).await?;
                    return Ok(nats_bin_path);
                }
                // Ignore LICENSE and README in the NATS tarball
                _ => (),
            }
        }
    }

    Err(anyhow!(
        "NATS Server binary could not be installed, please see logs"
    ))
}

/// Helper function to execute a NATS server binary with required wasmCloud arguments, e.g. JetStream
/// # Arguments
///
/// * `bin_path` - Path to the nats-server binary to execute
/// * `stderr` - Specify where NATS stderr logs should be written to. If logs aren't important, use std::process::Stdio::null()
/// * `port` - Port for NATS to listen on
pub async fn start_nats_server<P, T>(bin_path: P, stderr: T, port: u16) -> Result<Child>
where
    P: AsRef<Path>,
    T: Into<Stdio>,
{
    // If we can connect to the local port, NATS won't be able to listen on that port
    if tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .is_ok()
    {
        return Err(anyhow!(
            "Could not start NATS server, a process is already listening on 127.0.0.1:{}",
            port
        ));
    }
    Command::new(bin_path.as_ref())
        .stderr(stderr)
        .arg("-js")
        .arg("--addr")
        .arg("0.0.0.0")
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .map_err(|e| anyhow!(e))
}

/// Helper function to indicate if the NATS server binary is successfully
/// installed in a directory
pub async fn is_nats_installed<P>(dir: P) -> bool
where
    P: AsRef<Path>,
{
    metadata(dir.as_ref().join(NATS_SERVER_BINARY))
        .await
        .map_or(false, |m| m.is_file())
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
    format!(
        "{}/{}/nats-server-{}-{}-{}.tar.gz",
        NATS_GITHUB_RELEASE_URL, version, version, os, arch
    )
}

#[cfg(test)]
mod test {
    use crate::start::{
        ensure_nats_server, is_nats_installed, start_nats_server, NATS_SERVER_BINARY,
    };
    use anyhow::Result;
    use std::env::temp_dir;
    use tokio::fs::{create_dir_all, remove_dir_all};

    const NATS_SERVER_VERSION: &str = "v2.8.4";

    #[tokio::test]
    async fn can_handle_missing_nats_version() -> Result<()> {
        let install_dir = temp_dir().join("can_handle_missing_nats_version");
        let _ = remove_dir_all(&install_dir).await;
        create_dir_all(&install_dir).await?;
        assert!(!is_nats_installed(&install_dir).await);

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
        assert!(!is_nats_installed(&install_dir).await);

        let res = ensure_nats_server(NATS_SERVER_VERSION, &install_dir).await;
        assert!(res.is_ok());

        let log_path = install_dir.join("nats.log");
        let log_file = tokio::fs::File::create(&log_path).await?.into_std().await;

        let child_res =
            start_nats_server(&install_dir.join(NATS_SERVER_BINARY), log_file, 10000).await;
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
        assert!(!is_nats_installed(&install_dir).await);

        let res = ensure_nats_server(NATS_SERVER_VERSION, &install_dir).await;
        assert!(res.is_ok());

        let nats_one = start_nats_server(
            &install_dir.join(NATS_SERVER_BINARY),
            std::process::Stdio::null(),
            10003,
        )
        .await;
        assert!(nats_one.is_ok());

        // Give NATS a few seconds to start up and listen
        tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
        let log_path = install_dir.join("nats.log");
        let log = std::fs::File::create(&log_path)?;
        let nats_two = start_nats_server(&install_dir.join(NATS_SERVER_BINARY), log, 10003).await;
        assert!(nats_two.is_err());

        nats_one.unwrap().kill().await?;
        let _ = remove_dir_all(install_dir).await;

        Ok(())
    }
}

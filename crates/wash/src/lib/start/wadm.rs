use anyhow::{Context as _, Result};
use command_group::AsyncCommandGroup;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::fs::metadata;
use tokio::process::{Child, Command};
use tracing::warn;

use super::download_binary_from_github;
use crate::lib::common::CommandGroupUsage;

const WADM_GITHUB_RELEASE_URL: &str = "https://github.com/wasmcloud/wadm/releases/download";
pub const WADM_PID: &str = "wadm.pid";
#[cfg(target_family = "unix")]
pub const WADM_BINARY: &str = "wadm";
#[cfg(target_family = "windows")]
pub const WADM_BINARY: &str = "wadm.exe";

/// Downloads the wadm binary for the architecture and operating system of the current host machine.
///
/// # Arguments
///
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to download the `wadm` binary to
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use crate::lib::start::ensure_wadm;
/// let res = ensure_wadm("v0.4.0-alpha.1", "/tmp/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wadm");
/// # }
/// ```
pub async fn ensure_wadm<P>(version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    ensure_wadm_for_os_arch_pair(std::env::consts::OS, std::env::consts::ARCH, version, dir).await
}

/// Ensures the `wadm` binary is installed, returning the path to the executable early if it exists or
/// downloading the specified GitHub release version of wadm from <https://github.com/wasmcloud/wadm/releases/>
/// and unpacking the binary for a specified OS/ARCH pair to a directory. Returns the path to the wadm executable.
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to download the `wadm` binary to
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use crate::lib::start::ensure_wadm_for_os_arch_pair;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = ensure_wadm_for_os_arch_pair(os, arch, "v0.4.0-alpha.1", "/tmp/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wadm");
/// # }
/// ```
pub async fn ensure_wadm_for_os_arch_pair<P>(
    os: &str,
    arch: &str,
    version: &str,
    dir: P,
) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let wadm_bin_path = dir.as_ref().join(WADM_BINARY);
    if let Ok(_md) = metadata(&wadm_bin_path).await {
        // Check version to see if we need to download new one
        if let Ok(output) = Command::new(&wadm_bin_path).arg("--version").output().await {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            eprintln!("👀 Found wadm version on the disk: {}", stdout.trim_end());
            let re = regex::Regex::new(r"^wadm[^\s]*").unwrap();
            if re.replace(&stdout, "").to_string().trim() == version.trim_start_matches('v') {
                // wadm already exists, return early
                return Ok(wadm_bin_path);
            }
        }
    }
    // Download wadm tarball
    eprintln!(
        "🎣 Downloading new wadm from {}",
        &wadm_url(os, arch, version)
    );

    let res = download_binary_from_github(&wadm_url(os, arch, version), dir, WADM_BINARY).await;
    if let Ok(ref path) = res {
        eprintln!("🎯 Saved wadm to {}", path.display());
    }

    res
}

/// Downloads the wadm binary for the architecture and operating system of the current host machine.
///
/// # Arguments
///
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to download the `wadm` binary to
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use crate::lib::start::download_wadm;
/// let res = download_wadm("v0.4.0-alpha.1", "/tmp/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wadm");
/// # }
/// ```
pub async fn download_wadm<P>(version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    download_binary_from_github(
        &wadm_url(std::env::consts::OS, std::env::consts::ARCH, version),
        dir,
        WADM_BINARY,
    )
    .await
}

/// Configuration for wadm
#[derive(Clone)]
pub struct WadmConfig {
    /// Whether or not to use structured log output (as JSON)
    pub structured_logging: bool,
    /// The NATS `JetStream` domain to connect to [env: `WADM_JETSTREAM_DOMAIN`=]
    pub js_domain: Option<String>,
    /// The URL of the nats server you want to connect to
    pub nats_server_url: String,
    // (Optional) NATS credential file to use when authenticating [env: WADM_NATS_CREDS_FILE=]
    pub nats_credsfile: Option<PathBuf>,
}

/// Helper function to execute a wadm binary with optional arguments. This function does not check to see if a
/// wadm instance is already running or managing a lattice as wadm does not need to be a singleton.
///
/// # Arguments
///
/// * `state_dir` - Path to the folder in which wadm process state (ex. pidfile) should be stored
/// * `bin_path` - Path to the wadm binary to execute
/// * `stderr` - Specify where wadm stderr logs should be written to. If logs aren't important, use `std::process::Stdio::null()`
/// * `config` - Optional configuration for wadm
pub async fn start_wadm<T>(
    state_dir: impl AsRef<Path>,
    bin_path: impl AsRef<Path>,
    stderr: T,
    config: Option<WadmConfig>,
    command_group: CommandGroupUsage,
) -> Result<Child>
where
    T: Into<Stdio>,
{
    let mut cmd = Command::new(bin_path.as_ref());
    cmd.stderr(stderr).stdin(Stdio::null());

    if let Some(wadm_config) = config {
        cmd.arg("--nats-server");
        cmd.arg(wadm_config.nats_server_url);
        if wadm_config.structured_logging {
            cmd.arg("--structured-logging");
        }
        if let Some(domain) = wadm_config.js_domain.as_ref() {
            cmd.arg("-d");
            cmd.arg(domain);
        }
        if let Some(credsfile) = wadm_config.nats_credsfile.as_ref() {
            cmd.arg("--nats-creds-file");
            cmd.arg(credsfile);
        }
    }

    let child = if command_group == CommandGroupUsage::CreateNew {
        cmd.group_spawn().map_err(anyhow::Error::from)?.into_inner()
    } else {
        cmd.spawn().map_err(anyhow::Error::from)?
    };

    let pid = child
        .id()
        .context("unexpectedly missing pid for spawned process")?;

    let pid_path = state_dir.as_ref().join(WADM_PID);
    if let Err(e) = tokio::fs::write(pid_path, pid.to_string()).await {
        warn!("Couldn't write wadm pidfile: {e}");
    }

    Ok(child)
}

/// Helper function to determine the wadm release path given an os/arch and version
fn wadm_url(os: &str, arch: &str, version: &str) -> String {
    // Replace architecture to match wadm release naming scheme
    let arch = match arch {
        "x86_64" => "amd64",
        _ => arch,
    };
    format!("{WADM_GITHUB_RELEASE_URL}/{version}/wadm-{version}-{os}-{arch}.tar.gz")
}

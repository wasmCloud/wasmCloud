use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::fs::metadata;
use tokio::process::{Child, Command};
use tracing::warn;

use super::download_binary_from_github;

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
/// use wash_lib::start::ensure_wadm;
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
/// use wash_lib::start::ensure_wadm_for_os_arch_pair;
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
            if stdout.replace("wadm", "").trim() == version.trim_start_matches('v') {
                // wadm already exists, return early
                return Ok(wadm_bin_path);
            }
        }
    }
    // Download wadm tarball
    download_binary_from_github(&wadm_url(os, arch, version), dir, WADM_BINARY).await
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
/// use wash_lib::start::download_wadm;
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
    /// The NATS JetStream domain to connect to [env: WADM_JETSTREAM_DOMAIN=]
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
/// * `bin_path` - Path to the wadm binary to execute
/// * `stderr` - Specify where wadm stderr logs should be written to. If logs aren't important, use `std::process::Stdio::null()`
/// * `config` - Optional configuration for wadm
pub async fn start_wadm<P, T>(bin_path: P, stderr: T, config: Option<WadmConfig>) -> Result<Child>
where
    P: AsRef<Path>,
    T: Into<Stdio>,
{
    let pid_file = bin_path.as_ref().parent().map(|p| p.join(WADM_PID));

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

    let child = cmd.spawn().map_err(anyhow::Error::from);

    let pid = child.as_ref().map(Child::id);
    if let (Ok(Some(wadm_pid)), Some(pid_path)) = (pid, pid_file) {
        if let Err(e) = tokio::fs::write(pid_path, wadm_pid.to_string()).await {
            warn!("Couldn't write wadm pidfile: {e}");
        }
    }
    child
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

#[cfg(test)]
mod test {
    use crate::start::is_bin_installed;

    use super::*;
    use anyhow::Result;
    use std::env::temp_dir;
    use tokio::fs::{create_dir_all, remove_dir_all};

    const WADM_VERSION: &str = "v0.4.0-alpha.1";

    #[tokio::test]
    #[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
    async fn can_handle_missing_wadm_version() -> Result<()> {
        let install_dir = temp_dir().join("can_handle_missing_wadm_version");
        let _ = remove_dir_all(&install_dir).await;
        create_dir_all(&install_dir).await?;
        assert!(!is_bin_installed(&install_dir, WADM_BINARY).await);

        let major: u8 = 123;
        let minor: u8 = 52;
        let patch: u8 = 222;

        let res = ensure_wadm(&format!("v{major}.{minor}.{patch}"), &install_dir).await;
        assert!(res.is_err());

        let _ = remove_dir_all(install_dir).await;
        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
    async fn can_download_and_start_wadm() -> Result<()> {
        let install_dir = temp_dir().join("can_download_and_start_wadm");
        let _ = remove_dir_all(&install_dir).await;
        create_dir_all(&install_dir).await?;
        assert!(!is_bin_installed(&install_dir, WADM_BINARY).await);

        let res = ensure_wadm(WADM_VERSION, &install_dir).await;
        assert!(res.is_ok());

        let log_path = install_dir.join("wadm.log");
        let log_file = tokio::fs::File::create(&log_path).await?.into_std().await;

        let config = WadmConfig {
            structured_logging: false,
            js_domain: None,
            nats_server_url: "nats://127.0.0.1:54321".to_string(),
            nats_credsfile: None,
        };

        let child_res = start_wadm(&install_dir.join(WADM_BINARY), log_file, Some(config)).await;
        assert!(child_res.is_ok());

        // Wait for process to exit since NATS couldn't connect
        assert!(child_res.unwrap().wait().await.is_ok());
        let log_contents = tokio::fs::read_to_string(&log_path).await?;
        // wadm couldn't connect to NATS but that's okay

        // Different OS-es have different error codes, but all I care about is that wadm executed at all
        #[cfg(target_os = "macos")]
        assert!(log_contents.contains("Connection refused (os error 61)"));
        #[cfg(target_os = "linux")]
        assert!(log_contents.contains("Connection refused (os error 111)"));
        #[cfg(target_os = "windows")]
        assert!(log_contents.contains("No connection could be made because the target machine actively refused it. (os error 10061)"));

        let _ = remove_dir_all(install_dir).await;
        Ok(())
    }
}

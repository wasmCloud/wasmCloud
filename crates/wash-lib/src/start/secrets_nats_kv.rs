use anyhow::{Context as _, Result};
use command_group::AsyncCommandGroup;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::fs::metadata;
use tokio::process::{Child, Command};
use tracing::warn;

use super::download_binary_from_github;
use crate::common::CommandGroupUsage;

/// Release prefix that contains artifacts including `secrets-nats-kv`
const GITHUB_RELEASE_URL_PREFIX: &str = "https://github.com/wasmcloud/wasmcloud/releases/download";

const GITHUB_RELEASE_ARTIFACT_PREFIX: &str = "secrets-nats-kv";

const VERSION_TAG_PREFIX: &str = "secrets-nats-kv";

pub const PID_FILENAME: &str = "secrets-nats-kv.pid";

#[cfg(target_family = "unix")]
pub const BINARY_FILENAME: &str = "secrets-nats-kv";
#[cfg(target_family = "windows")]
pub const BINARY_FILENAME: &str = "secrets-nats-kv.exe";

/// Helper function to generate the download URL for a released version
///
/// # Arguments
///
/// * `os` - operating system (ex. `linux`, `darwin`, `windows`)
/// * `arch` - architecture (ex. `amd64`)
/// * `version` - version (ex. `v0.1.1`)
///
fn artifact_download_url(os: &str, arch: &str, version: &str) -> String {
    // Replace architecture to match release naming scheme
    let arch = match arch {
        "x86_64" => "amd64",
        _ => arch,
    };
    let extension = if os == "windows" { ".exe" } else { "" };
    format!("{GITHUB_RELEASE_URL_PREFIX}/{VERSION_TAG_PREFIX}-{version}/{GITHUB_RELEASE_ARTIFACT_PREFIX}-{arch}-{os}{extension}")
}

/// Downloads the NATS KV secrets binary for the architecture and operating system of the current host machine.
///
/// # Arguments
///
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to download the binary to
///
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::nats_kv_secrets::ensure_binary;
/// let res = ensure_binary("v0.1.1", "/tmp/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/secrets-nats-kv");
/// # }
/// ```
pub async fn ensure_binary<P>(version: &str, install_dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    ensure_for_os_arch_pair(
        std::env::consts::OS,
        std::env::consts::ARCH,
        version,
        install_dir,
    )
    .await
}

/// Ensures the `secrets-nats-kv` binary is installed, returning the path to the executable early if it exists or
/// downloading the specified GitHub release version of secrets-nats-kv from <https://github.com/wasmcloud/wasmcloud/releases/>
/// and unpacking the binary for a specified OS/ARCH pair to a directory.
///
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to download the binary to
///
/// # Returns
///
/// Returns the path to the secrets-nats-kv executable.
///
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::ensure_for_os_arch_pair;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = ensure_for_os_arch_pair(os, arch, "v0.1.1", "/tmp/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/secrets-nats-kv");
/// # }
/// ```
///
pub async fn ensure_for_os_arch_pair<P>(
    os: &str,
    arch: &str,
    version: &str,
    dir: P,
) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let bin_path = dir.as_ref().join(BINARY_FILENAME);
    if let Ok(_md) = metadata(&bin_path).await {
        // Check version to see if we need to download new one
        if let Ok(output) = Command::new(&bin_path).arg("--version").output().await {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            eprintln!(
                "👀 Found secrets-nats-kv version on the disk: {}",
                stdout.trim_end()
            );
            let re = regex::Regex::new(r"^secrets-nats-kv[^\s]*").unwrap();
            if re.replace(&stdout, "").to_string().trim() == version.trim_start_matches('v') {
                eprintln!("✅ Using secrets-nats-kv version [{}]", &version);
                return Ok(bin_path);
            }
        }
    }
    // Download tarball
    eprintln!(
        "🎣 Downloading new secrets-nats-kv from {}",
        &artifact_download_url(os, arch, version)
    );

    let res = download_binary_from_github(
        &artifact_download_url(os, arch, version),
        dir,
        BINARY_FILENAME,
    )
    .await;
    if let Ok(ref path) = res {
        eprintln!("🎯 Saved secrets-nats-kv to {}", path.display());
    }

    res
}

/// Downloads the binary for the architecture and operating system of the current host machine.
///
/// # Arguments
///
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to download the binary to
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::download_binary;
/// let res = download_binary("v0.1.1", "/tmp/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/secrets-nats-kv");
/// # }
/// ```
pub async fn download_binary<P>(version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    download_binary_from_github(
        &artifact_download_url(std::env::consts::OS, std::env::consts::ARCH, version),
        dir,
        BINARY_FILENAME,
    )
    .await
}

/// Configuration that must be translated to CLI invocations of `secrets-nats-kv run`
#[derive(Default, Clone)]
pub struct Config {
    /// The name of the secrets backend, defaults to `nats-kv`
    pub secrets_backend_name: Option<String>,

    /// The NATS KV bucket to use for storing secrets
    pub secrets_bucket: Option<String>,

    /// The NATS address to connect to where the backend is running
    pub nats_address: Option<String>,

    /// The API version to use for the secrets backend
    pub secrets_api_version: Option<String>,

    /// NATS credentials file path
    pub nats_creds_file: Option<PathBuf>,

    /// XKey seed for use during transit
    pub transit_xkey_seed: Option<String>,

    /// XKey seed for use during encryption at rest
    pub encryption_xkey_seed: Option<String>,
}

/// Helper function to execute the binary binary with optional arguments.
///
/// # Arguments
///
/// * `state_dir` - Path to the folder in which process state (ex. pidfile) can be stored
/// * `bin_path` - Path to the binary to execute
/// * `stderr` - Specify where stderr logs should be written to. If logs aren't important, use `std::process::Stdio::null()`
/// * `config` - Optional configuration
///
pub async fn start_binary<P, T>(
    state_dir: P,
    bin_path: P,
    stderr: T,
    config: Option<Config>,
    command_group: CommandGroupUsage,
) -> Result<Child>
where
    P: AsRef<Path>,
    T: Into<Stdio>,
{
    let pid_file = state_dir.as_ref().parent().map(|p| p.join(PID_FILENAME));

    // Build configuration with bin path
    let mut cmd = Command::new(bin_path.as_ref());
    cmd.arg("run");
    cmd.stderr(stderr).stdin(Stdio::null());

    // Apply config options for secrets-nats-kv
    if let Some(Config {
        secrets_backend_name,
        secrets_bucket,
        nats_address,
        secrets_api_version,
        nats_creds_file,
        encryption_xkey_seed,
        transit_xkey_seed,
    }) = config
    {
        if let Some(name) = secrets_backend_name {
            cmd.args(["--secrets-backend-name", &name]);
        }
        if let Some(bucket) = secrets_bucket {
            cmd.args(["--secrets-bucket", &bucket]);
        }
        if let Some(addr) = nats_address {
            cmd.args(["--nats-address", &addr]);
        }
        if let Some(v) = secrets_api_version {
            cmd.args(["--secrets-api-version", &v]);
        }
        if let Some(p) = nats_creds_file {
            cmd.args(["--nats-creds-file", &format!("{}", p.display())]);
        }
        if let Some(k) = transit_xkey_seed {
            cmd.env("TRANSIT_XKEY_SEED", k);
        }
        if let Some(k) = encryption_xkey_seed {
            cmd.env("ENCRYPTION_XKEY_SEED", k);
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
    if let Some(pid_path) = pid_file {
        if let Err(e) = tokio::fs::write(pid_path, pid.to_string()).await {
            warn!("Couldn't write pidfile: {e}");
        }
    }

    Ok(child)
}

#[cfg(test)]
mod test {
    use anyhow::Result;

    use crate::common::CommandGroupUsage;
    use crate::start::is_bin_installed;

    use super::*;

    const NATS_KV_SECRETS_VERSION: &str = "v0.1.1-rc.0";

    /// Ensure that nats-secrets-kv binary can be downloaded
    #[tokio::test]
    #[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
    async fn can_download_nats_kv_secrets() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        assert!(!is_bin_installed(temp_dir.path(), BINARY_FILENAME).await);
        download_binary(NATS_KV_SECRETS_VERSION, temp_dir.path()).await?;
        assert!(is_bin_installed(temp_dir.path(), BINARY_FILENAME).await);
        Ok(())
    }

    /// Ensure that attempting to install a missing version fails
    #[tokio::test]
    #[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
    async fn can_handle_missing_nats_kv_secrets_version() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        assert!(!is_bin_installed(temp_dir.path(), BINARY_FILENAME).await);
        assert!(ensure_binary("v123.52.22", temp_dir.path()).await.is_err());
        Ok(())
    }

    /// Ensure that we can download and start the binary with configuration
    #[tokio::test]
    #[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
    async fn can_download_and_start_nats_kv_secrets() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let install_dir = temp_dir.path();

        assert!(!is_bin_installed(install_dir, BINARY_FILENAME).await);

        assert!(ensure_binary(NATS_KV_SECRETS_VERSION, install_dir)
            .await
            .is_ok());

        let pid_path = install_dir.join(PID_FILENAME);
        let log_path = install_dir.join("secrets-nats-kv.log");
        let log_file = tokio::fs::File::create(&log_path).await?.into_std().await;

        let child_res = start_binary(
            install_dir,
            install_dir.join(BINARY_FILENAME).as_path(),
            log_file,
            None,
            CommandGroupUsage::UseParent,
        )
        .await;
        assert!(child_res.is_ok());

        // Starting the KV secrets provider will fail, but that's OK, we care only that it was able to at least start.
        assert!(child_res.unwrap().wait().await.is_ok());
        assert!(tokio::fs::try_exists(&log_path).await.is_ok());
        assert!(tokio::fs::try_exists(&pid_path).await.is_ok());

        Ok(())
    }
}

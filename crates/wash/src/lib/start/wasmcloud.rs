use std::collections::HashMap;

use anyhow::{bail, Result};
use reqwest::StatusCode;
use semver::Version;
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
/// use crate::lib::start::ensure_wasmcloud;
/// let res = ensure_wasmcloud(&semver::Version::parse("0.63.0").unwrap(), "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/v0.63.0/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn ensure_wasmcloud<P>(version: &Version, dir: P) -> Result<PathBuf>
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
///   directory where to store hosts. Each host will be stored in a directory matching its version
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use crate::lib::start::ensure_wasmcloud_for_os_arch_pair;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = ensure_wasmcloud_for_os_arch_pair(&semver::Version::parse("0.63.0").unwrap(), "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/v0.63.0/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn ensure_wasmcloud_for_os_arch_pair<P>(version: &Version, dir: P) -> Result<PathBuf>
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
///   directory where to store hosts. Each host will be stored in a directory matching its version
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use crate::lib::start::download_wasmcloud;
/// let res = download_wasmcloud(&semver::Version::parse("0.57.1").unwrap(), "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/v0.63.0/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn download_wasmcloud<P>(version: &Version, dir: P) -> Result<PathBuf>
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
///   directory where to store hosts. Each host will be stored in a directory matching its version
/// # Examples
///
/// ```rust,ignore
/// # #[tokio::main]
/// # async fn main() {
/// use crate::lib::start::download_wasmcloud_for_os_arch_pair;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = download_wasmcloud_for_os_arch_pair(&semver::Version::parse("0.63.0").unwrap(), "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/v0.63.0/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn download_wasmcloud_for_os_arch_pair<P>(version: &Version, dir: P) -> Result<PathBuf>
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
    let version_dir = dir.as_ref().join(format!("v{version}"));
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
pub async fn find_wasmcloud_binary<P>(dir: P, version: &Version) -> Option<PathBuf>
where
    P: AsRef<Path>,
{
    let versioned_dir = dir.as_ref().join(format!("v{version}"));
    let bin_file = versioned_dir.join(WASMCLOUD_HOST_BIN);

    metadata(&bin_file).await.is_ok().then_some(bin_file)
}

/// Helper function to determine the wasmCloud host release path given an os/arch and version
fn wasmcloud_url(version: &Version) -> String {
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
        "{WASMCLOUD_GITHUB_RELEASE_URL}/v{version}/wasmcloud-{arch}-{os}",
        arch = std::env::consts::ARCH,
    )
}

/// Helper function to ensure the version of wasmCloud is above the minimum
/// supported version (v0.63.0) that runs burrito releases
fn check_version(version: &Version) -> Result<()> {
    let version_req = semver::VersionReq::parse(&format!(">={MINIMUM_WASMCLOUD_VERSION}"))?;
    if !version.pre.is_empty() {
        warn!("Using prerelease version {} of wasmCloud", version);
        return Ok(());
    }

    if !version_req.matches(version) {
        bail!(
            "wasmCloud version v{version} is earlier than the minimum supported version of v{MINIMUM_WASMCLOUD_VERSION}",
        );
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::{check_version, MINIMUM_WASMCLOUD_VERSION};
    use semver::Version;

    #[tokio::test]
    async fn can_properly_deny_too_old_hosts() -> anyhow::Result<()> {
        // Ensure we allow versions >= 0.81.0
        assert!(check_version(&Version::parse("0.81.0")?).is_ok());
        assert!(check_version(&Version::parse(MINIMUM_WASMCLOUD_VERSION)?).is_ok());

        // Ensure we allow prerelease tags for testing
        assert!(check_version(&Version::parse("0.81.0-rc1")?).is_ok());

        // Ensure we deny versions < MINIMUM_WASMCLOUD_VERSION
        assert!(check_version(&Version::parse("0.80.99")?).is_err());

        if let Err(e) = check_version(&Version::parse("0.56.0")?) {
            assert_eq!(e.to_string(), format!("wasmCloud version v0.56.0 is earlier than the minimum supported version of v{MINIMUM_WASMCLOUD_VERSION}"));
        } else {
            panic!("v0.56.0 should be before the minimum version")
        }

        Ok(())
    }
}

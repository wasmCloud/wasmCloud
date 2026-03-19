//! Command for self-updating the `wash` CLI tool

use anyhow::Context as _;
use clap::Args;
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT},
};
use semver::Version;
use serde::Deserialize;
use serde_json::json;
use std::ops::Deref;
use tracing::{debug, error, instrument, trace, warn};

#[cfg(unix)]
use std::fs::Permissions;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use tokio::{fs, io::AsyncWriteExt};

use crate::cli::{CliCommand, CliContext, CommandOutput};

const REPO: &str = "wasmcloud/wasmCloud";
const BINARY_NAME: &str = "wash";

#[derive(Debug, Deserialize)]
/// Represents a GitHub release with its tag name and assets
pub struct Release {
    pub tag_name: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
/// An asset in a GitHub release, containing its name and download URL
pub struct Asset {
    pub id: usize,
    pub name: String,
}

/// CLI command for updating wash to the latest version
///
/// # Examples
///
/// Update from default public repository:
/// ```bash
/// wash update
/// ```
///
/// Update from private repository with token:
/// ```bash
/// wash update --git myorg/my-private-wash --token ghp_xxxxxxxxxxxx
/// ```
///
/// Update from private repository using environment variable:
/// ```bash
/// export GITHUB_TOKEN=ghp_xxxxxxxxxxxx
/// wash update --git myorg/my-private-wash
/// ```
#[derive(Args, Debug, Default, Clone)]
pub struct UpdateCommand {
    /// Force update even if already on the latest version
    #[arg(long, short = 'f')]
    force: bool,

    /// Check for updates without applying them
    #[arg(long, short = 'd')]
    dry_run: bool,

    /// Point at a different repository for updates
    #[arg(long, default_value = REPO)]
    git: String,

    /// GitHub token for private repository access. Can also be set via GITHUB_TOKEN, GH_TOKEN, or GITHUB_ACCESS_TOKEN environment variables
    #[arg(long, env = "WASH_GITHUB_TOKEN")]
    token: Option<String>,
}

fn parse_version(tag: &str) -> Option<Version> {
    let t = tag.strip_prefix("wash-").unwrap_or(tag);
    let t = t.strip_prefix('v').unwrap_or(t);

    match Version::parse(t) {
        Ok(version) => {
            trace!("Parsed version '{}' as {}", tag, version);
            Some(version)
        }
        Err(e) => {
            trace!("Failed to parse version '{}': {}", tag, e);
            None
        }
    }
}

impl CliCommand for UpdateCommand {
    #[instrument(level = "debug", skip_all, name = "update")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let config = UpdateConfig::new(self.git.clone(), self.token.clone());
        let (os, arch) = get_os_arch();

        // Check current version and constraints
        let current_version = if let Some(current) = self.get_current_version() {
            current
        } else {
            anyhow::bail!("Cannot determine current wash version");
        };

        debug!("Current wash version: {}", current_version);

        let release = self
            .fetch_latest_release(&config)
            .await
            .context("failed to fetch latest release")?;
        let asset = find_asset(&release.assets, os, arch).ok_or_else(|| {
            anyhow::anyhow!("No matching binary found in release assets for {arch}-{os}",)
        })?;

        let should_update = if let Some(latest_version) = parse_version(&release.tag_name) {
            debug!("Latest wash version: {}", latest_version);
            latest_version > current_version
        } else {
            anyhow::bail!(
                "Cannot parse version from latest release tag: {}",
                release.tag_name
            );
        };

        // Handle dry-run mode
        if self.dry_run {
            if should_update {
                return Ok(CommandOutput::ok(
                    format!(
                        "Update available. Local version: {}, Latest version: {}",
                        current_version, release.tag_name
                    ),
                    Some(json!({
                        "current_version": current_version.to_string(),
                        "target_version": release.tag_name,
                        "update_available": should_update,
                        "dry_run": true
                    })),
                ));
            } else {
                return Ok(CommandOutput::ok(
                    format!(
                        "No update needed. Local version: {}, Latest version: {}",
                        current_version, release.tag_name
                    ),
                    Some(json!({
                        "current_version": current_version.to_string(),
                        "target_version": release.tag_name,
                        "update_available": should_update,
                        "dry_run": true
                    })),
                ));
            }
        }

        if !should_update && !self.force {
            return Ok(CommandOutput::ok(
                format!("wash is already up to date (version {})", release.tag_name),
                Some(json!({
                    "current_version": current_version.to_string(),
                    "target_version": release.tag_name,
                    "update_available": should_update,
                })),
            ));
        }

        let binary_bytes = self.download_asset(asset.id, &config).await?;

        let current_wash = std::env::current_exe();
        let cache_dir = ctx.deref().in_cache_dir("update");
        tokio::fs::create_dir_all(&cache_dir).await?;

        let install_path = if let Ok(current_wash) = current_wash {
            let backup_path = cache_dir.join("wash_backup");
            tokio::fs::copy(&current_wash, &backup_path).await?;
            debug!(
                backup_path = ?backup_path.display(),
                "backing up current wash binary"
            );

            // On unix, need to write to a temp file and then atomically replace
            let tmp_path = current_wash.with_extension("tmp_upgrade");
            {
                let mut f = fs::File::create(&tmp_path).await?;
                f.write_all(&binary_bytes).await?;
                debug!(
                    path = ?tmp_path.display(),
                    "wrote new wash binary to temporary file",
                );
            }
            #[cfg(unix)]
            {
                tokio::fs::set_permissions(&tmp_path, Permissions::from_mode(0o755)).await?;
                trace!(
                    ?tmp_path,
                    "set permissions for new wash binary to 755 (rwxr-xr-x)"
                );
            }

            match tokio::fs::copy(&tmp_path, &current_wash).await {
                Ok(_) => {
                    debug!(
                        ?current_wash,
                        "successfully replaced current wash binary with new version"
                    );
                    current_wash
                }
                Err(e) => {
                    error!(
                        ?e,
                        "failed to replace current wash binary, referencing temporary file instead"
                    );
                    tmp_path
                }
            }
        } else {
            warn!(
                "Cannot find installed wash binary, assuming installation in a non-standard location."
            );
            let install_path = ctx.in_data_dir(&format!("{BINARY_NAME}_{os}_{arch}"));
            tokio::fs::create_dir_all(ctx.data_dir())
                .await
                .context("failed to create data directory")?;
            tokio::fs::write(&install_path, &binary_bytes)
                .await
                .context("failed to write new wash binary")?;
            install_path
        };

        Ok(CommandOutput::ok(
            format!("wash upgraded to {tag_name}", tag_name = release.tag_name),
            Some(json!(
                {
                    "version": release.tag_name,
                    "backup_path": cache_dir.join("wash_backup").display().to_string(),
                    "install_path": install_path.display().to_string(),
                }
            )),
        ))
    }
}

impl UpdateCommand {
    /// Get the current version of wash
    fn get_current_version(&self) -> Option<Version> {
        let version = env!("CARGO_PKG_VERSION");
        parse_version(version)
    }

    /// Fetch the latest release from the configured repository with authentication
    async fn fetch_latest_release(&self, config: &UpdateConfig) -> anyhow::Result<Release> {
        let url = format!(
            "https://api.github.com/repos/{}/releases/latest",
            config.repo
        );
        let client = config.create_client()?;

        debug!(repo = %config.repo, "Fetching latest release");
        let resp = client.get(url).send().await?.error_for_status()?;
        Ok(resp.json().await?)
    }

    /// Download an asset with authentication if configured
    async fn download_asset(&self, id: usize, config: &UpdateConfig) -> anyhow::Result<Vec<u8>> {
        let client = config.create_client()?;
        let url = format!(
            "https://api.github.com/repos/{}/releases/assets/{}",
            config.repo, id
        );
        debug!(url = %url, "Downloading asset");
        let resp = client
            .get(url)
            .header("Accept", "application/octet-stream")
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.bytes().await?.to_vec())
    }
}

/// Simple version check function for internal use (public repository only)
pub async fn fetch_latest_release_public() -> anyhow::Result<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let client = Client::builder().user_agent("wash-self-upgrade").build()?;
    let resp = client.get(url).send().await?.error_for_status()?;
    Ok(resp.json().await?)
}

fn get_os_arch() -> (&'static str, &'static str) {
    let arch = std::env::consts::ARCH;
    let arch = match arch {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        _ => {
            warn!(
                arch,
                "unsupported architecture for update, will likely fail"
            );
            arch
        }
    };

    let os = std::env::consts::OS;
    let os = match os {
        "macos" | "darwin" => "apple-darwin",
        "linux" => "unknown-linux-musl",
        _ => {
            warn!(os, "unsupported os for update, will likely fail");
            os
        }
    };

    (os, arch)
}

fn find_asset<'a>(assets: &'a [Asset], os: &str, arch: &str) -> Option<&'a Asset> {
    let expected = if os == "windows" {
        format!("{BINARY_NAME}-{arch}-{os}.exe")
    } else {
        format!("{BINARY_NAME}-{arch}-{os}")
    };
    trace!(?expected, "looking for asset in release with name");
    assets.iter().find(|a| a.name == expected)
}

/// Configuration for GitHub authentication and repository access
#[derive(Debug, Clone)]
struct UpdateConfig {
    repo: String,
    token: Option<String>,
}

impl UpdateConfig {
    /// Creates a new UpdateConfig with token resolution from multiple sources
    fn new(repo: String, token: Option<String>) -> Self {
        if token.is_some() {
            debug!("GitHub token found for private repository access");
        }

        Self { repo, token }
    }

    /// Creates an authenticated HTTP client
    fn create_client(&self) -> anyhow::Result<Client> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("wash-self-upgrade"));

        if let Some(ref token) = self.token {
            let auth_header = format!("Bearer {token}");
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&auth_header)
                    .context("Failed to create authorization header")?,
            );
            debug!("Using GitHub token for authentication");
        }

        Client::builder()
            .default_headers(headers)
            .build()
            .context("Failed to create HTTP client")
    }
}

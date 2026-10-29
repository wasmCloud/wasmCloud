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
use std::{ops::Deref, path::Path};
use tracing::{debug, instrument, trace, warn};

#[cfg(unix)]
use std::fs::Permissions;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use tokio::fs;

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
    #[arg(long, env = "WASH_GITHUB_TOKEN", hide_env_values = true)]
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
        self.handle_with(ctx, &config, std::env::current_exe).await
    }
}

impl UpdateCommand {
    async fn handle_with(
        &self,
        ctx: &CliContext,
        config: &UpdateConfig,
        current_exe: impl FnOnce() -> std::io::Result<std::path::PathBuf>,
    ) -> anyhow::Result<CommandOutput> {
        let (os, arch) = get_os_arch();

        // Check current version and constraints
        let current_version = if let Some(current) = self.get_current_version() {
            current
        } else {
            anyhow::bail!("Cannot determine current wash version");
        };

        debug!("Current wash version: {}", current_version);

        let release = config
            .fetch_latest_release()
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

        let binary_bytes = config.download_asset(asset.id).await?;

        let current_wash = current_exe();
        let cache_dir = ctx.deref().in_cache_dir("update");
        tokio::fs::create_dir_all(&cache_dir).await?;

        let install_path = if let Ok(current_wash) = current_wash {
            let backup_path = cache_dir.join("wash_backup");
            tokio::fs::copy(&current_wash, &backup_path).await?;
            debug!(
                backup_path = ?backup_path.display(),
                "backing up current wash binary"
            );

            replace_executable(&current_wash, &binary_bytes).await?;
            debug!(
                ?current_wash,
                "successfully replaced current wash binary with new version"
            );
            current_wash
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

    /// Get the current version of wash
    fn get_current_version(&self) -> Option<Version> {
        let version = env!("CARGO_PKG_VERSION");
        parse_version(version)
    }
}

/// Simple version check function for internal use (public repository only)
pub async fn fetch_latest_release_public() -> anyhow::Result<Release> {
    UpdateConfig::new(REPO.into(), None)
        .fetch_latest_release()
        .await
}

/// Installs `bytes` at `executable` by renaming a sibling temporary file over
/// it. A rename swaps the directory entry instead of writing into the running
/// image, which Linux refuses with `ETXTBSY`, and resolving the path first
/// replaces a symlinked install at its target instead of the link itself.
async fn replace_executable(executable: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let target = fs::canonicalize(executable).await?;
    let tmp_path = target.with_extension("tmp_upgrade");
    let replaced = async {
        fs::write(&tmp_path, bytes).await?;
        #[cfg(unix)]
        fs::set_permissions(&tmp_path, Permissions::from_mode(0o755)).await?;
        trace!(?tmp_path, "wrote new wash binary to temporary file");
        swap_executable(&tmp_path, &target).await
    }
    .await;
    if replaced.is_err() {
        let _ = fs::remove_file(&tmp_path).await;
    }
    replaced.with_context(|| format!("failed to replace {}", executable.display()))
}

#[cfg(not(windows))]
async fn swap_executable(tmp_path: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(tmp_path, target).await
}

/// Windows renames a running executable but will not replace one, so the
/// current image moves aside first; the next update removes it.
#[cfg(windows)]
async fn swap_executable(tmp_path: &Path, target: &Path) -> std::io::Result<()> {
    let previous = target.with_extension("old");
    let _ = fs::remove_file(&previous).await;
    fs::rename(target, &previous).await?;
    let swapped = fs::rename(tmp_path, target).await;
    if swapped.is_err() {
        let _ = fs::rename(&previous, target).await;
    }
    swapped
}

fn get_os_arch() -> (&'static str, &'static str) {
    normalize_os_arch(std::env::consts::OS, std::env::consts::ARCH)
}

fn normalize_os_arch<'a>(os: &'a str, arch: &'a str) -> (&'a str, &'a str) {
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
    api_base_url: String,
}

impl UpdateConfig {
    /// Creates a new UpdateConfig with token resolution from multiple sources
    fn new(repo: String, token: Option<String>) -> Self {
        if token.is_some() {
            debug!("GitHub token found for private repository access");
        }

        Self {
            repo,
            token,
            api_base_url: "https://api.github.com".into(),
        }
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
                    .context("failed to create authorization header")?,
            );
            debug!("Using GitHub token for authentication");
        }

        Client::builder()
            .default_headers(headers)
            .build()
            .context("failed to create HTTP client")
    }

    /// Fetch the latest release from the configured repository with authentication
    async fn fetch_latest_release(&self) -> anyhow::Result<Release> {
        let url = format!("{}/repos/{}/releases/latest", self.api_base_url, self.repo);
        let client = self.create_client()?;

        debug!(repo = %self.repo, "Fetching latest release");
        let resp = client.get(url).send().await?.error_for_status()?;
        Ok(resp.json().await?)
    }

    /// Download an asset with authentication if configured
    async fn download_asset(&self, id: usize) -> anyhow::Result<Vec<u8>> {
        let client = self.create_client()?;
        let url = format!(
            "{}/repos/{}/releases/assets/{}",
            self.api_base_url, self.repo, id
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::cli::test_support::{HttpFixture, Response, context};
    use anyhow::Result;
    use clap::Parser;
    use std::{io, path::PathBuf, time::Duration};
    use tokio::time::timeout;

    const WAIT: Duration = Duration::from_secs(10);
    const NEW_BINARY: &[u8] = b"\0wash\xff\r\nnew binary";

    #[derive(Parser)]
    struct UpdateCli {
        #[command(flatten)]
        command: UpdateCommand,
    }

    struct Server {
        config: UpdateConfig,
        fixture: HttpFixture,
    }

    impl Server {
        async fn start(responses: Vec<(u16, Vec<u8>)>, token: Option<&str>) -> Result<Self> {
            let responses = responses
                .into_iter()
                .map(|(status, body)| Response::new(status, body))
                .collect();
            Self::start_with(responses, token).await
        }

        async fn start_with(responses: Vec<Response>, token: Option<&str>) -> Result<Self> {
            wash_runtime::init_crypto();
            let fixture = HttpFixture::start(responses).await?;
            let mut config =
                UpdateConfig::new("example/private-wash".into(), token.map(str::to_owned));
            config.api_base_url = format!("http://{}", fixture.address);
            Ok(Self { config, fixture })
        }

        async fn finish(&mut self, expected_paths: &[&str], token: Option<&str>) -> Result<()> {
            let requests = self.fixture.finish().await?;
            assert_eq!(requests.len(), expected_paths.len());
            for (request, path) in requests.iter().zip(expected_paths) {
                assert_eq!(
                    request.target,
                    format!("GET /repos/example/private-wash{path} HTTP/1.1")
                );
                assert_eq!(
                    request.headers.get(USER_AGENT),
                    Some(&HeaderValue::from_static("wash-self-upgrade"))
                );
                assert_eq!(
                    request
                        .headers
                        .get(AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    token.map(|token| format!("Bearer {token}")).as_deref()
                );
                if path.starts_with("/releases/assets/") {
                    assert_eq!(
                        request.headers.get("accept"),
                        Some(&HeaderValue::from_static("application/octet-stream"))
                    );
                }
            }
            Ok(())
        }
    }

    fn release(tag: &str) -> Vec<u8> {
        let (os, arch) = get_os_arch();
        let suffix = if os == "windows" { ".exe" } else { "" };
        json!({
            "tag_name": tag,
            "assets": [
                {"id": 1, "name": "wash-wrong-target"},
                {"id": 42, "name": format!("wash-{arch}-{os}{suffix}")}
            ]
        })
        .to_string()
        .into_bytes()
    }

    fn newer_tag() -> Result<String> {
        let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
        Ok(format!("wash-v{}.0.0", current.major + 1))
    }

    fn unavailable_exe() -> io::Result<PathBuf> {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no executable path",
        ))
    }

    #[test]
    fn version_tags_accept_supported_prefixes_and_preserve_semver() -> Result<()> {
        for prefix in ["", "v", "wash-", "wash-v"] {
            for version in ["0.0.0", "2.11.0", "12.34.56-alpha.1+build.009"] {
                assert_eq!(
                    parse_version(&format!("{prefix}{version}")),
                    Some(Version::parse(version)?)
                );
            }
        }
        for tag in [
            "",
            "latest",
            "2",
            "2.11",
            "2.11.0.1",
            "02.11.0",
            "2.11.0-01",
            "2.11.0-",
            "2.11.0+",
            " 2.11.0",
            "2.11.0\n",
            "V2.11.0",
            "wash-wash-v2.11.0",
            "vv2.11.0",
            "vwash-2.11.0",
        ] {
            assert_eq!(parse_version(tag), None, "accepted invalid tag {tag:?}");
        }
        assert_eq!(
            UpdateCommand::default().get_current_version(),
            Some(Version::parse(env!("CARGO_PKG_VERSION"))?)
        );
        Ok(())
    }

    #[test]
    fn platform_names_normalize_aliases_and_preserve_unknown_targets() {
        for (os, expected_os) in [
            ("macos", "apple-darwin"),
            ("darwin", "apple-darwin"),
            ("linux", "unknown-linux-musl"),
            ("windows", "windows"),
            ("freebsd", "freebsd"),
        ] {
            for (arch, expected_arch) in [
                ("x86_64", "x86_64"),
                ("amd64", "x86_64"),
                ("aarch64", "aarch64"),
                ("arm64", "aarch64"),
                ("riscv64", "riscv64"),
            ] {
                assert_eq!(normalize_os_arch(os, arch), (expected_os, expected_arch));
            }
        }
        assert_eq!(
            get_os_arch(),
            normalize_os_arch(std::env::consts::OS, std::env::consts::ARCH)
        );
    }

    #[test]
    fn asset_selection_requires_an_exact_platform_binary_name() {
        for (os, arch, name) in [
            ("apple-darwin", "aarch64", "wash-aarch64-apple-darwin"),
            ("apple-darwin", "x86_64", "wash-x86_64-apple-darwin"),
            (
                "unknown-linux-musl",
                "aarch64",
                "wash-aarch64-unknown-linux-musl",
            ),
            (
                "unknown-linux-musl",
                "x86_64",
                "wash-x86_64-unknown-linux-musl",
            ),
            ("windows", "x86_64", "wash-x86_64-windows.exe"),
        ] {
            let mut assets: Vec<Asset> = [
                format!("{name}.sha256"),
                format!("{name}.tar.gz"),
                format!("{name}.sig"),
                format!("prefix-{name}"),
                name.to_uppercase(),
                "wash".into(),
                "wash-x86_64-pc-windows-msvc.exe".into(),
            ]
            .into_iter()
            .enumerate()
            .map(|(id, name)| Asset { id, name })
            .collect();
            if os == "windows" {
                assets.push(Asset {
                    id: 10,
                    name: name.trim_end_matches(".exe").into(),
                });
            } else {
                assets.push(Asset {
                    id: 10,
                    name: format!("{name}.exe"),
                });
            }
            assert!(find_asset(&assets, os, arch).is_none());
            assets.push(Asset {
                id: 42,
                name: name.into(),
            });
            assets.push(Asset {
                id: 43,
                name: name.into(),
            });
            assert_eq!(
                find_asset(&assets, os, arch).map(|asset| asset.id),
                Some(42)
            );
        }
        assert!(find_asset(&[], "apple-darwin", "aarch64").is_none());
    }

    #[test]
    fn cli_parses_defaults_and_long_and_short_flags() -> Result<()> {
        // Override the token so the invoking shell cannot choose the test input.
        let defaults = UpdateCli::try_parse_from(["update", "--token", "test-token"])?.command;
        assert!(!defaults.force);
        assert!(!defaults.dry_run);
        assert_eq!(defaults.git, REPO);
        assert_eq!(defaults.token.as_deref(), Some("test-token"));
        for flags in [["--force", "--dry-run"], ["-f", "-d"]] {
            let command = UpdateCli::try_parse_from([
                "update",
                flags[0],
                flags[1],
                "--git",
                "org/private",
                "--token",
                "test-token",
            ])?
            .command;
            assert!(command.force && command.dry_run);
            assert_eq!(command.git, "org/private");
        }
        for args in [
            vec!["update", "--git"],
            vec!["update", "--token"],
            vec!["update", "--unknown"],
        ] {
            assert!(UpdateCli::try_parse_from(args).is_err());
        }
        Ok(())
    }

    #[tokio::test]
    async fn dry_run_reports_version_ordering_without_installing_even_when_forced() -> Result<()> {
        let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
        for (tag, available) in [
            (newer_tag()?, true),
            (format!("{}-rc.1", newer_tag()?), true),
            (format!("wash-v{current}"), false),
            ("wash-v0.0.0".into(), false),
            (format!("wash-v{current}-rc.1"), false),
        ] {
            for force in [false, true] {
                let temp = tempfile::tempdir()?;
                let mut server = Server::start(vec![(200, release(&tag))], None).await?;
                let command = UpdateCommand {
                    dry_run: true,
                    force,
                    ..Default::default()
                };
                let output = timeout(
                    WAIT,
                    command.handle_with(&context(temp.path()), &server.config, || {
                        unreachable!("dry-run must not inspect the executable")
                    }),
                )
                .await??;
                assert!(output.is_success());
                assert_eq!(
                    output.json(),
                    Some(
                        &json!({"current_version": current.to_string(), "target_version": tag, "update_available": available, "dry_run": true})
                    )
                );
                let expected = if available {
                    "Update available."
                } else {
                    "No update needed."
                };
                assert_eq!(
                    output.text(),
                    format!("{expected} Local version: {current}, Latest version: {tag}")
                );
                assert_eq!(std::fs::read_dir(temp.path())?.count(), 0);
                server.finish(&["/releases/latest"], None).await?;
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn up_to_date_and_older_releases_do_not_download_or_install() -> Result<()> {
        for tag in [env!("CARGO_PKG_VERSION"), "wash-v0.0.0"] {
            let temp = tempfile::tempdir()?;
            let mut server = Server::start(vec![(200, release(tag))], None).await?;
            let output = timeout(
                WAIT,
                UpdateCommand::default().handle_with(&context(temp.path()), &server.config, || {
                    unreachable!("no-op update must not inspect the executable")
                }),
            )
            .await??;
            assert!(output.is_success());
            assert_eq!(
                output.text(),
                format!("wash is already up to date (version {tag})")
            );
            assert_eq!(
                output.json(),
                Some(
                    &json!({"current_version": env!("CARGO_PKG_VERSION"), "target_version": tag, "update_available": false})
                )
            );
            assert_eq!(std::fs::read_dir(temp.path())?.count(), 0);
            server.finish(&["/releases/latest"], None).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn updates_back_up_the_original_and_install_exact_downloaded_bytes() -> Result<()> {
        for (force, tag) in [
            (false, newer_tag()?),
            (true, env!("CARGO_PKG_VERSION").into()),
            (true, "wash-v0.0.0".into()),
        ] {
            let temp = tempfile::tempdir()?;
            let ctx = context(temp.path());
            let executable = temp.path().join("wash.exe");
            fs::write(&executable, b"old binary").await?;
            let mut server = Server::start(
                vec![(200, release(&tag)), (200, NEW_BINARY.to_vec())],
                Some("private-token"),
            )
            .await?;
            let command = UpdateCommand {
                force,
                ..Default::default()
            };
            let output = timeout(
                WAIT,
                command.handle_with(&ctx, &server.config, || Ok(executable.clone())),
            )
            .await??;
            assert!(output.is_success());
            assert_eq!(output.text(), format!("wash upgraded to {tag}"));
            assert_eq!(
                output.json(),
                Some(
                    &json!({"version": tag, "backup_path": ctx.in_cache_dir("update/wash_backup").display().to_string(), "install_path": executable.display().to_string()})
                )
            );
            assert_eq!(fs::read(&executable).await?, NEW_BINARY);
            assert_eq!(
                fs::read(ctx.in_cache_dir("update/wash_backup")).await?,
                b"old binary"
            );
            assert!(!executable.with_extension("tmp_upgrade").exists());
            #[cfg(unix)]
            assert_eq!(
                fs::metadata(&executable).await?.permissions().mode() & 0o777,
                0o755
            );
            #[cfg(windows)]
            assert_eq!(
                fs::read(executable.with_extension("old")).await?,
                b"old binary"
            );
            server
                .finish(
                    &["/releases/latest", "/releases/assets/42"],
                    Some("private-token"),
                )
                .await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn missing_executable_installs_in_the_data_directory() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let tag = newer_tag()?;
        let mut server =
            Server::start(vec![(200, release(&tag)), (200, NEW_BINARY.to_vec())], None).await?;
        let output = timeout(
            WAIT,
            UpdateCommand::default().handle_with(&ctx, &server.config, unavailable_exe),
        )
        .await??;
        let (os, arch) = get_os_arch();
        let installed = ctx.in_data_dir(&format!("wash_{os}_{arch}"));
        assert!(output.is_success());
        assert_eq!(fs::read(&installed).await?, NEW_BINARY);
        assert_eq!(
            output.json(),
            Some(
                &json!({"version": tag, "backup_path": ctx.in_cache_dir("update/wash_backup").display().to_string(), "install_path": installed.display().to_string()})
            )
        );
        assert!(!ctx.in_cache_dir("update/wash_backup").exists());
        server
            .finish(&["/releases/latest", "/releases/assets/42"], None)
            .await?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_installs_are_replaced_at_their_target() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let target = temp.path().join("wash-real");
        fs::write(&target, b"original").await?;
        let link = temp.path().join("wash");
        std::os::unix::fs::symlink(&target, &link)?;
        let mut server = Server::start(
            vec![(200, release(&newer_tag()?)), (200, NEW_BINARY.to_vec())],
            None,
        )
        .await?;
        let output = timeout(
            WAIT,
            UpdateCommand::default().handle_with(&ctx, &server.config, || Ok(link.clone())),
        )
        .await??;
        assert!(output.is_success());
        assert!(fs::symlink_metadata(&link).await?.is_symlink());
        assert_eq!(fs::read(&target).await?, NEW_BINARY);
        assert!(!target.with_extension("tmp_upgrade").exists());
        server
            .finish(&["/releases/latest", "/releases/assets/42"], None)
            .await?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unwritable_install_directory_fails_and_keeps_the_original() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).await?;
        let executable = bin.join("wash");
        fs::write(&executable, b"original").await?;
        fs::set_permissions(&bin, Permissions::from_mode(0o555)).await?;
        // Root ignores directory permissions, so there is no failure to observe.
        if fs::write(bin.join("probe"), b"").await.is_ok() {
            fs::set_permissions(&bin, Permissions::from_mode(0o755)).await?;
            return Ok(());
        }
        let mut server = Server::start(
            vec![(200, release(&newer_tag()?)), (200, NEW_BINARY.to_vec())],
            None,
        )
        .await?;
        let result = timeout(
            WAIT,
            UpdateCommand::default().handle_with(&ctx, &server.config, || Ok(executable.clone())),
        )
        .await?;
        fs::set_permissions(&bin, Permissions::from_mode(0o755)).await?;
        let error = result.err().context("expected replacement error")?;
        assert_eq!(
            error.to_string(),
            format!("failed to replace {}", executable.display())
        );
        assert_eq!(
            error.downcast_ref::<io::Error>().map(io::Error::kind),
            Some(io::ErrorKind::PermissionDenied)
        );
        assert_eq!(fs::read(&executable).await?, b"original");
        assert!(!executable.with_extension("tmp_upgrade").exists());
        assert_eq!(
            fs::read(ctx.in_cache_dir("update/wash_backup")).await?,
            b"original"
        );
        server
            .finish(&["/releases/latest", "/releases/assets/42"], None)
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn invalid_release_metadata_and_http_errors_leave_files_untouched() -> Result<()> {
        let tag = newer_tag()?;
        let (os, arch) = get_os_arch();
        for (status, body, expected) in [
            (
                401,
                b"unauthorized".to_vec(),
                "failed to fetch latest release".to_owned(),
            ),
            (
                404,
                b"not found".to_vec(),
                "failed to fetch latest release".to_owned(),
            ),
            (
                500,
                b"server error".to_vec(),
                "failed to fetch latest release".to_owned(),
            ),
            (
                200,
                b"not json".to_vec(),
                "failed to fetch latest release".to_owned(),
            ),
            (
                200,
                b"{\"tag_name\":\"v1.0.0\"}".to_vec(),
                "failed to fetch latest release".to_owned(),
            ),
            (
                200,
                serde_json::to_vec(&json!({"tag_name": tag, "assets": []}))?,
                format!("No matching binary found in release assets for {arch}-{os}"),
            ),
            (
                200,
                release("not-a-version"),
                "Cannot parse version from latest release tag: not-a-version".to_owned(),
            ),
        ] {
            for dry_run in [false, true] {
                let temp = tempfile::tempdir()?;
                let mut server = Server::start(vec![(status, body.clone())], None).await?;
                let command = UpdateCommand {
                    dry_run,
                    force: true,
                    ..Default::default()
                };
                let error = timeout(
                    WAIT,
                    command.handle_with(&context(temp.path()), &server.config, || {
                        unreachable!("invalid metadata must not reach installation")
                    }),
                )
                .await?
                .err()
                .context("expected release error")?;
                assert_eq!(error.to_string(), expected);
                assert_eq!(std::fs::read_dir(temp.path())?.count(), 0);
                server.finish(&["/releases/latest"], None).await?;
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn failed_download_never_backs_up_or_replaces_the_executable() -> Result<()> {
        for status in [403, 404, 500] {
            let temp = tempfile::tempdir()?;
            let ctx = context(temp.path());
            let executable = temp.path().join("wash");
            fs::write(&executable, b"original").await?;
            let mut server = Server::start(
                vec![(200, release(&newer_tag()?)), (status, b"error".to_vec())],
                None,
            )
            .await?;
            let error = timeout(
                WAIT,
                UpdateCommand::default().handle_with(&ctx, &server.config, || {
                    unreachable!("failed download must not reach installation")
                }),
            )
            .await?
            .err()
            .context("expected download error")?;
            assert_eq!(
                error
                    .downcast_ref::<reqwest::Error>()
                    .and_then(reqwest::Error::status)
                    .map(|status| status.as_u16()),
                Some(status)
            );
            assert_eq!(fs::read(&executable).await?, b"original");
            assert!(!ctx.cache_dir().exists());
            assert!(!executable.with_extension("tmp_upgrade").exists());
            server
                .finish(&["/releases/latest", "/releases/assets/42"], None)
                .await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn truncated_responses_fail_before_installation() -> Result<()> {
        for truncate_release in [false, true] {
            let temp = tempfile::tempdir()?;
            let ctx = context(temp.path());
            let metadata = release(&newer_tag()?);
            let length = metadata.len();
            let responses = if truncate_release {
                vec![Response {
                    status: 200,
                    body: metadata,
                    content_length: length + 100,
                }]
            } else {
                vec![
                    Response::new(200, metadata),
                    Response {
                        status: 200,
                        body: b"partial".to_vec(),
                        content_length: 100,
                    },
                ]
            };
            let mut server = Server::start_with(responses, None).await?;
            let error = timeout(
                WAIT,
                UpdateCommand::default().handle_with(&ctx, &server.config, || {
                    unreachable!("truncated response must not reach installation")
                }),
            )
            .await?
            .err()
            .context("expected incomplete response error")?;
            assert!(error.downcast_ref::<reqwest::Error>().is_some());
            if truncate_release {
                assert_eq!(error.to_string(), "failed to fetch latest release");
            }
            assert_eq!(std::fs::read_dir(temp.path())?.count(), 0);
            let paths = if truncate_release {
                vec!["/releases/latest"]
            } else {
                vec!["/releases/latest", "/releases/assets/42"]
            };
            server.finish(&paths, None).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_disappeared_executable_aborts_installation_without_a_backup() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let missing = temp.path().join("missing-wash");
        let mut server = Server::start(
            vec![(200, release(&newer_tag()?)), (200, NEW_BINARY.to_vec())],
            None,
        )
        .await?;
        let error = timeout(
            WAIT,
            UpdateCommand::default().handle_with(&ctx, &server.config, || Ok(missing.clone())),
        )
        .await?
        .err()
        .context("expected backup error")?;
        assert_eq!(
            error.downcast_ref::<io::Error>().map(io::Error::kind),
            Some(io::ErrorKind::NotFound)
        );
        assert!(!ctx.in_cache_dir("update/wash_backup").exists());
        assert!(!ctx.data_dir().exists());
        assert!(!missing.with_extension("tmp_upgrade").exists());
        server
            .finish(&["/releases/latest", "/releases/assets/42"], None)
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn filesystem_failures_propagate_without_overwriting_the_original() -> Result<()> {
        for failure in [
            "cache",
            "backup",
            "temporary",
            "data-directory",
            "data-file",
        ] {
            let temp = tempfile::tempdir()?;
            let ctx = context(temp.path());
            let executable = temp.path().join("wash");
            fs::write(&executable, b"original").await?;
            match failure {
                "cache" => fs::write(ctx.cache_dir(), b"block directory creation").await?,
                "backup" => fs::create_dir_all(ctx.in_cache_dir("update/wash_backup")).await?,
                "temporary" => fs::create_dir(executable.with_extension("tmp_upgrade")).await?,
                "data-directory" => fs::write(ctx.data_dir(), b"block directory creation").await?,
                "data-file" => {
                    let (os, arch) = get_os_arch();
                    fs::create_dir_all(ctx.in_data_dir(&format!("wash_{os}_{arch}"))).await?;
                }
                _ => unreachable!(),
            }
            let mut server = Server::start(
                vec![(200, release(&newer_tag()?)), (200, NEW_BINARY.to_vec())],
                None,
            )
            .await?;
            let error = timeout(
                WAIT,
                UpdateCommand::default().handle_with(&ctx, &server.config, || {
                    if failure.starts_with("data-") {
                        unavailable_exe()
                    } else {
                        Ok(executable.clone())
                    }
                }),
            )
            .await?
            .err()
            .context("expected filesystem error")?;
            assert!(error.downcast_ref::<io::Error>().is_some());
            if failure == "data-directory" {
                assert_eq!(error.to_string(), "failed to create data directory");
            } else if failure == "data-file" {
                assert_eq!(error.to_string(), "failed to write new wash binary");
            }
            assert_eq!(
                fs::read(&executable).await?,
                b"original",
                "failure at {failure}"
            );
            server
                .finish(&["/releases/latest", "/releases/assets/42"], None)
                .await?;
        }
        Ok(())
    }

    #[test]
    fn release_deserialization_rejects_missing_or_malformed_asset_fields() {
        for value in [
            json!({"tag_name": "wash-v1.0.0"}),
            json!({"tag_name": null, "assets": []}),
            json!({"tag_name": "wash-v1.0.0", "assets": null}),
            json!({"tag_name": "wash-v1.0.0", "assets": [{"name": "wash"}]}),
            json!({"tag_name": "wash-v1.0.0", "assets": [{"id": 1}]}),
            json!({"tag_name": "wash-v1.0.0", "assets": [{"id": -1, "name": "wash"}]}),
        ] {
            assert!(
                serde_json::from_value::<Release>(value.clone()).is_err(),
                "accepted {value}"
            );
        }
    }

    #[tokio::test]
    async fn command_handle_preserves_authentication_errors_before_any_io() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let command = UpdateCommand {
            git: "example/private-wash".into(),
            token: Some("invalid\nheader".into()),
            ..Default::default()
        };
        let error = command
            .handle(&context(temp.path()))
            .await
            .err()
            .context("expected invalid token error")?;
        assert_eq!(error.to_string(), "failed to fetch latest release");
        assert!(
            error
                .chain()
                .any(|cause| cause.to_string() == "failed to create authorization header")
        );
        assert_eq!(std::fs::read_dir(temp.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn invalid_authentication_headers_fail_with_context() {
        wash_runtime::init_crypto();
        for token in [
            "token\nInjected: header",
            "token\r\nInjected: header",
            "token\0",
        ] {
            let config = UpdateConfig::new(REPO.into(), Some(token.into()));
            let error = config.create_client().err();
            assert_eq!(
                error.as_ref().map(ToString::to_string).as_deref(),
                Some("failed to create authorization header")
            );
        }
        let config = UpdateConfig::new(REPO.into(), None);
        assert_eq!(config.repo, REPO);
        assert_eq!(config.api_base_url, "https://api.github.com");
        assert!(config.token.is_none());
    }
}

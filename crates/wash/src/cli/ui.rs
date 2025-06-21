use crate::lib::{
    cli::{CommandOutput, OutputKind},
    common::{DEFAULT_WASH_UI_PORT, WASHBOARD_VERSION, WASHBOARD_VERSION_T},
    config::WASH_DIRECTORIES,
    start::{
        get_download_client, new_patch_or_pre_1_0_0_minor_version_after_version_string,
        parse_version_string, GITHUB_WASHBOARD_TAG_PREFIX, GITHUB_WASMCLOUD_ORG,
        GITHUB_WASMCLOUD_TS_REPO,
    },
};
use anyhow::{bail, Context, Result};
use async_compression::tokio::bufread::GzipDecoder;
use clap::Parser;
use semver::Version;
use std::{io::Cursor, path::PathBuf};
use tokio_tar::Archive;
use tracing::debug;
use warp::Filter;

#[derive(Parser, Debug, Clone)]
pub struct UiCommand {
    /// Which port to run the UI on, defaults to 3030
    #[clap(short = 'p', long = "port", default_value = DEFAULT_WASH_UI_PORT, env = "WASMCLOUD_WASH_UI_PORT")]
    pub port: u16,

    /// Which version of the UI to run
    #[clap(short = 'v', long = "version", env = "WASMCLOUD_WASHBOARD_VERSION")]
    pub version: Option<String>,
}

pub async fn handle_command(command: UiCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    handle_ui(command, output_kind)
        .await
        .map(|()| (CommandOutput::default()))
}

async fn get_patch_version_or_default(version: Option<String>) -> Version {
    if let Some(version) = parse_version_string(version) {
        return version;
    }

    match new_patch_or_pre_1_0_0_minor_version_after_version_string(
        GITHUB_WASMCLOUD_ORG,
        GITHUB_WASMCLOUD_TS_REPO,
        WASHBOARD_VERSION,
        Some(GITHUB_WASHBOARD_TAG_PREFIX),
    )
    .await
    {
        Ok(new_patch_version) => {
            debug!("Found new patch version: {new_patch_version}");
            new_patch_version
        }
        _ => {
            debug!("No new version found, using default version: {WASHBOARD_VERSION}");
            WASHBOARD_VERSION_T
        }
    }
}

pub async fn handle_ui(cmd: UiCommand, _output_kind: OutputKind) -> Result<()> {
    let washboard_version = get_patch_version_or_default(cmd.version).await;
    let washboard_path = WASH_DIRECTORIES.in_downloads_dir("washboard");
    let washboard_assets = ensure_washboard(&washboard_version, washboard_path).await?;
    let static_files = warp::fs::dir(washboard_assets);
    let washboard_port = cmd.port;

    let cors = warp::cors()
        .allow_any_origin()
        .allow_methods(vec!["GET", "POST"])
        .allow_headers(vec!["Content-Type"]);

    eprintln!("washboard-ui@{washboard_version} running on http://localhost:{washboard_port}");
    eprintln!("Hit CTRL-C to stop");

    warp::serve(static_files.with(cors))
        .run(([127, 0, 0, 1], washboard_port))
        .await;

    Ok(())
}

async fn ensure_washboard(version: &Version, base_dir: PathBuf) -> Result<PathBuf> {
    let install_dir = base_dir.join(version.to_string());

    if tokio::fs::metadata(&install_dir).await.is_err() {
        download_washboard(version, &install_dir).await?;
    }

    Ok(install_dir)
}

async fn download_washboard(version: &Version, install_dir: &PathBuf) -> Result<()> {
    let urls = vec![
        format!(
            "https://github.com/wasmCloud/typescript/releases/download/washboard-ui%40{version}/washboard.tar.gz"
        ),
        format!(
            "https://github.com/wasmCloud/wasmCloud/releases/download/typescript%2Fapps%2Fwashboard-ui%2Fv{version}/washboard.tar.gz"
        ),
        format!(
            "https://github.com/wasmCloud/wasmCloud/releases/download/washboard-ui-v{version}/washboard.tar.gz"
        ),
    ];

    let body = try_download_from_urls(&urls)
        .await
        .context("Failed to download washboard-ui assets")?;

    eprintln!("Downloaded washboard-ui@{version}");

    // Unpack and copy to install dir
    let cursor = Cursor::new(body);
    let mut tarball = Archive::new(Box::new(GzipDecoder::new(cursor)));
    tarball
        .unpack(install_dir)
        .await
        .context("Failed to unpack washboard-ui assets")?;

    Ok(())
}

async fn try_download_from_urls(urls: &[String]) -> Result<bytes::Bytes> {
    let mut last_error = None;

    for url in urls {
        match try_download(url).await {
            Ok(body) => return Ok(body),
            Err(e) => last_error = Some(e),
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Failed to find suitable download URL")))
}

async fn try_download(url: &String) -> Result<bytes::Bytes> {
    let resp = get_download_client()?
        .get(url)
        .send()
        .await
        .with_context(|| {
            format!("Failed to get response from request to download washboard release from: {url}")
        })?;

    match resp.status() {
        err if err != reqwest::StatusCode::OK => {
            bail!("Failed to download washboard release from {url}: {err}");
        }
        _ => {}
    }

    resp.bytes()
        .await
        .with_context(|| format!("Failed to read response body from request to: {url}"))
}

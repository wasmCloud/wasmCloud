mod config;
pub use config::*;

use std::{io::Cursor, path::PathBuf};

use anyhow::{bail, Context, Result};
use async_compression::tokio::bufread::GzipDecoder;
use clap::Parser;
use tokio_tar::Archive;
use warp::Filter;
use crate::lib::{
    cli::{CommandOutput, OutputKind},
    config::downloads_dir,
    start::get_download_client,
};

const DEFAULT_WASHBOARD_VERSION: &str = "v0.6.0";

#[derive(Parser, Debug, Clone)]
pub struct UiCommand {
    /// Which port to run the UI on, defaults to 3030
    #[clap(short = 'p', long = "port", default_value = DEFAULT_WASH_UI_PORT)]
    pub port: u16,

    /// Which version of the UI to run
    #[clap(short = 'v', long = "version", default_value = DEFAULT_WASHBOARD_VERSION)]
    pub version: String,
}

pub async fn handle_command(command: UiCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    handle_ui(command, output_kind)
        .await
        .map(|()| (CommandOutput::default()))
}

pub async fn handle_ui(cmd: UiCommand, _output_kind: OutputKind) -> Result<()> {
    let washboard_path = downloads_dir()?.join("washboard");
    let washboard_assets = ensure_washboard(&cmd.version, washboard_path).await?;
    let static_files = warp::fs::dir(washboard_assets);

    let cors = warp::cors()
        .allow_any_origin()
        .allow_methods(vec!["GET", "POST"])
        .allow_headers(vec!["Content-Type"]);

    eprintln!(
        "washboard-ui@{} running on http://localhost:{}",
        cmd.version, cmd.port
    );
    eprintln!("Hit CTRL-C to stop");

    warp::serve(static_files.with(cors))
        .run(([127, 0, 0, 1], cmd.port))
        .await;

    Ok(())
}

async fn ensure_washboard(version: &str, base_dir: PathBuf) -> Result<PathBuf> {
    let install_dir = base_dir.join(version);

    if tokio::fs::metadata(&install_dir).await.is_err() {
        download_washboard(version, &install_dir).await?;
    }

    Ok(install_dir)
}

async fn download_washboard(version: &str, install_dir: &PathBuf) -> Result<()> {
    let urls = vec![
        format!(
            "https://github.com/wasmCloud/typescript/releases/download/washboard-ui%40{version}/washboard.tar.gz"
        ),
        format!(
            "https://github.com/wasmCloud/wasmCloud/releases/download/typescript%2Fapps%2Fwashboard-ui%2F{version}/washboard.tar.gz"
        ),
        format!(
            "https://github.com/wasmCloud/wasmCloud/releases/download/washboard-ui-{version}/washboard.tar.gz"
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
        .context("Failed to download washboard.tar.gz. Are you offline?")?;

    if resp.status() != reqwest::StatusCode::OK {
        bail!("Failed to download washboard.tar.gz: {}", resp.status());
    }

    resp.bytes().await.context(
        "Failed to read bytes from washboard.tar.gz. Try deleting the download and try again.",
    )
}

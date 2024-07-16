mod config;
pub use config::*;

use std::{io::Cursor, path::PathBuf};

use anyhow::{bail, Context, Result};
use async_compression::tokio::bufread::GzipDecoder;
use clap::Parser;
use tokio_tar::Archive;
use warp::Filter;
use wash_lib::{
    cli::{CommandOutput, OutputKind},
    config::downloads_dir,
};
use wasmcloud_core::tls;

const DEFAULT_WASHBOARD_VERSION: &str = "v0.4.0";

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
        .map(|_| (CommandOutput::default()))
}

pub async fn handle_ui(cmd: UiCommand, _output_kind: OutputKind) -> Result<()> {
    let washboard_path = downloads_dir()?.join("washboard");
    let washboard_assets = ensure_washboard(&cmd.version, washboard_path).await?;
    let static_files = warp::fs::dir(washboard_assets);

    let cors = warp::cors()
        .allow_any_origin()
        .allow_methods(vec!["GET", "POST"])
        .allow_headers(vec!["Content-Type"]);

    eprintln!("Washboard running on http://localhost:{}", cmd.port);
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
    let release_url = format!(
        "https://github.com/wasmCloud/wasmCloud/releases/download/washboard-ui-{version}/washboard.tar.gz"
    );

    // Download tarball
    let resp = tls::DEFAULT_REQWEST_CLIENT
        .get(&release_url)
        .send()
        .await
        .context("failed to request washboard tarball")?;

    if resp.status() != reqwest::StatusCode::OK {
        bail!("failed to download washboard tarball: {}", resp.status());
    }

    let body = resp
        .bytes()
        .await
        .context("failed to read bytes from washboard tarball")?;

    // Unpack and copy to install dir
    let cursor = Cursor::new(body);
    let mut tarball = Archive::new(Box::new(GzipDecoder::new(cursor)));
    tarball
        .unpack(install_dir)
        .await
        .context("failed to unpack washboard tarball")?;

    Ok(())
}

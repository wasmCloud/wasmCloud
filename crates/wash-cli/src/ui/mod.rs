use anyhow::Result;
use clap::Parser;
use rust_embed::RustEmbed;
use warp::Filter;

use wash_lib::cli::{CommandOutput, OutputKind};

mod config;
pub use config::*;

#[derive(RustEmbed)]
#[folder = "washboard/dist"]
struct Asset;

#[derive(Parser, Debug, Clone)]
pub struct UiCommand {
    /// Whist port to run the UI on, defaults to 3030
    #[clap(short = 'p', long = "port", default_value = DEFAULT_WASH_UI_PORT)]
    pub port: u16,
}

pub async fn handle_command(command: UiCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    handle_ui(command, output_kind)
        .await
        .map(|_| (CommandOutput::default()))
}

pub async fn handle_ui(cmd: UiCommand, _output_kind: OutputKind) -> Result<()> {
    let static_files = warp::any()
        .and(warp::get())
        .and(warp_embed::embed(&Asset))
        .boxed();

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

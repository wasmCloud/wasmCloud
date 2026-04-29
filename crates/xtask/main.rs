mod build_test_fixtures;

fn main() -> anyhow::Result<()> {
    dotenv_flow::dotenv_flow().ok();
    tracing_subscriber::fmt().init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(main_main())
}

use clap::builder::styling::AnsiColor;

const CLAP_STYLE: clap::builder::Styles = clap::builder::Styles::styled()
    .header(AnsiColor::Yellow.on_default())
    .usage(AnsiColor::Green.on_default())
    .literal(AnsiColor::Green.on_default())
    .placeholder(AnsiColor::Green.on_default());

#[derive(Debug, clap::Parser)]
#[clap(
    version,
    about,
    styles = CLAP_STYLE
)]
struct Args {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    BuildTestFixtures {},
}

async fn main_main() -> anyhow::Result<()> {
    let _cwd = std::env::current_dir()?;

    use clap::Parser;
    let args = Args::parse();
    match args.command {
        Commands::BuildTestFixtures {} => build_test_fixtures::run(),
    }
    Ok(())
}

use std::process::Command;
fn show_cmd(cmd: &mut Command) -> &mut Command {
    tracing::info!(
        "[{:?}, {:?} ]",
        cmd.get_program(),
        cmd.get_args().collect::<Vec<_>>()
    );
    cmd
}
fn cargo_cmd() -> Command {
    /* let mut cargo_bin = Command::new(
        std::env::var("XTASK_CARGO_BIN").unwrap_or_else(|_| "cargo".into()),
    ); */
    Command::new("cargo")
}

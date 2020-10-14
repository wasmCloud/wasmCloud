use std::path::PathBuf;
use structopt::StructOpt;

/// This renders appropriately with escape characters
const ASCII: &str = "
               __    ___   ___   __ _          _ _ 
__      ____ _/ _\\  / __\\ / __\\ / _\\ |__   ___| | |
\\ \\ /\\ / / _` \\ \\  / /   / /    \\ \\| '_ \\ / _ \\ | |
 \\ V  V / (_| |\\ \\/ /___/ /___  _\\ \\ | | |  __/ | |
  \\_/\\_/ \\__,_\\__/\\____/\\____/  \\__/_| |_|\\___|_|_|

A single CLI to handle all of your waSCC tooling needs
";

#[derive(Debug, Clone, StructOpt)]
#[structopt(name = "wash", about = ASCII)]
struct Cli {
    #[structopt(flatten)]
    command: CliCommand,
}

#[derive(Debug, Clone, StructOpt)]
enum CliCommand {
    /// claims
    #[structopt(name = "claims")]
    Claims(ClaimsCommand),
    /// keys
    #[structopt(name = "keys")]
    Keys(KeysCommand),
    /// reg
    #[structopt(name = "reg")]
    Reg(RegCommand),
    /// lattice
    #[structopt(name = "lattice")]
    Lattice(LatticeCommand),
}

#[derive(Debug, Clone, StructOpt)]
struct ClaimsCommand {
    /// Sample path
    #[structopt(short = "p", long = "path")]
    path: PathBuf,
}

#[derive(Debug, Clone, StructOpt)]
struct KeysCommand {
    /// Sample path
    #[structopt(short = "p", long = "path")]
    path: PathBuf,
}

#[derive(Debug, Clone, StructOpt)]
struct LatticeCommand {
    /// Sample path
    #[structopt(short = "p", long = "path")]
    path: PathBuf,
}

#[derive(Debug, Clone, StructOpt)]
struct RegCommand {
    /// Sample path
    #[structopt(short = "p", long = "path")]
    path: PathBuf,
}

fn main() {
    let cli = Cli::from_args();
    println!("{:#?}", cli);
}

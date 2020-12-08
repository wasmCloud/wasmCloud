use structopt::clap::AppSettings;
use structopt::StructOpt;

mod claims;
use claims::ClaimsCli;
mod lattice;
use lattice::LatticeCli;
mod keys;
use keys::KeysCli;
mod par;
use par::ParCli;
mod reg;
use reg::RegCli;
mod up;
use up::UpCli;

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
#[structopt(global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands, AppSettings::DisableHelpSubcommand]),
            name = "wash",
            about = ASCII)]
struct Cli {
    #[structopt(flatten)]
    command: CliCommand,
}

#[derive(Debug, Clone, StructOpt)]
enum CliCommand {
    /// Utilities for generating and managing JWTs for waSCC Actors
    #[structopt(name = "claims")]
    Claims(ClaimsCli),
    /// Utilities for generating and managing keys
    #[structopt(name = "keys", aliases = &["key"])]
    Keys(KeysCli),
    /// Utilities for interacting with a waSCC Lattice
    #[structopt(name = "lattice")]
    Lattice(LatticeCli),
    /// Utilities for creating, inspecting, and modifying capability provider archive files
    #[structopt(name = "par")]
    Par(ParCli),
    /// Utilities for interacting with OCI compliant registries
    #[structopt(name = "reg")]
    Reg(RegCli),
    /// Utility to launch waSCC REPL environment
    #[structopt(name = "up")]
    Up(UpCli),
}

fn main() {
    let cli = Cli::from_args();
    // env_logger::init();

    let res = match cli.command {
        CliCommand::Keys(keyscli) => keys::handle_command(keyscli),
        CliCommand::Lattice(latticecli) => lattice::handle_command(latticecli),
        CliCommand::Claims(claimscli) => claims::handle_command(claimscli),
        CliCommand::Par(parcli) => par::handle_command(parcli),
        CliCommand::Reg(regcli) => reg::handle_command(regcli),
        CliCommand::Up(upcli) => up::handle_command(upcli),
    };

    match res {
        Ok(_v) => (),
        Err(e) => println!("Error: {}", e),
    }
}

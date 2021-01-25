use structopt::clap::AppSettings;
use structopt::StructOpt;

mod claims;
use claims::ClaimsCli;
mod ctl;
use ctl::CtlCli;
mod keys;
use keys::KeysCli;
mod par;
use par::ParCli;
mod reg;
use reg::RegCli;
mod up;
use up::UpCli;
mod util;

/// This renders appropriately with escape characters
const ASCII: &str = "
                               _____ _                 _    _____ _          _ _ 
                              / ____| |               | |  / ____| |        | | |
 __      ____ _ ___ _ __ ___ | |    | | ___  _   _  __| | | (___ | |__   ___| | |
 \\ \\ /\\ / / _` / __| '_ ` _ \\| |    | |/ _ \\| | | |/ _` |  \\___ \\| '_ \\ / _ \\ | |
  \\ V  V / (_| \\__ \\ | | | | | |____| | (_) | |_| | (_| |  ____) | | | |  __/ | |
   \\_/\\_/ \\__,_|___/_| |_| |_|\\_____|_|\\___/ \\__,_|\\__,_| |_____/|_| |_|\\___|_|_|

A single CLI to handle all of your wasmCloud tooling needs
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
    /// Generate and manage JWTs for wasmCloud Actors
    #[structopt(name = "claims")]
    Claims(ClaimsCli),
    /// Utilities for generating and managing keys
    #[structopt(name = "keys", aliases = &["key"])]
    Keys(KeysCli),
    /// Interact with a wasmCloud control interface
    #[structopt(name = "ctl")]
    Ctl(CtlCli),
    /// Create, inspect, and modify capability provider archive files
    #[structopt(name = "par")]
    Par(ParCli),
    /// Interact with OCI compliant registries
    #[structopt(name = "reg")]
    Reg(RegCli),
    /// Launch wasmCloud REPL environment
    #[structopt(name = "up")]
    Up(UpCli),
}

#[actix_rt::main]
async fn main() {
    let cli = Cli::from_args();

    let res = match cli.command {
        CliCommand::Keys(keyscli) => keys::handle_command(keyscli.command()),
        CliCommand::Claims(claimscli) => claims::handle_command(claimscli.command()).await,
        CliCommand::Ctl(ctlcli) => ctl::handle_command(ctlcli.command()).await,
        CliCommand::Par(parcli) => par::handle_command(parcli.command()).await,
        CliCommand::Reg(regcli) => reg::handle_command(regcli.command()).await,
        CliCommand::Up(upcli) => up::handle_command(upcli.command())
            .await
            .map(|_s| "Exiting REPL".to_string()),
    };

    match res {
        Ok(out) => println!("{}", out),
        Err(e) => eprintln!("Error: {}", e),
    }
}

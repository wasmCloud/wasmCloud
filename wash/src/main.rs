use call::CallCli;
use claims::ClaimsCli;
use ctl::CtlCli;
use ctx::CtxCli;
use drain::DrainCli;
use generate::NewCli;
use keys::KeysCli;
use par::ParCli;
use reg::RegCli;
use smithy::{GenerateCli, LintCli, ValidateCli};
use structopt::{clap::AppSettings, StructOpt};

mod call;
mod claims;
mod ctl;
mod ctx;
mod drain;
mod generate;
mod id;
mod keys;
mod par;
mod reg;
mod smithy;
mod util;

const ASCII: &str = r#"
                               _____ _                 _    _____ _          _ _
                              / ____| |               | |  / ____| |        | | |
 __      ____ _ ___ _ __ ___ | |    | | ___  _   _  __| | | (___ | |__   ___| | |
 \ \ /\ / / _` / __| '_ ` _ \| |    | |/ _ \| | | |/ _` |  \___ \| '_ \ / _ \ | |
  \ V  V / (_| \__ \ | | | | | |____| | (_) | |_| | (_| |  ____) | | | |  __/ | |
   \_/\_/ \__,_|___/_| |_| |_|\_____|_|\___/ \__,_|\__,_| |_____/|_| |_|\___|_|_|

A single CLI to handle all of your wasmCloud tooling needs
"#;

#[derive(Debug, Clone, StructOpt)]
#[structopt(global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands, AppSettings::DisableHelpSubcommand]),
            name = "wash",
            about = ASCII)]
struct Cli {
    #[structopt(flatten)]
    command: CliCommand,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, StructOpt)]
enum CliCommand {
    /// Invoke a wasmCloud actor
    #[structopt(name = "call")]
    Call(CallCli),
    /// Generate and manage JWTs for wasmCloud actors
    #[structopt(name = "claims")]
    Claims(Box<ClaimsCli>),
    /// Interact with a wasmCloud control interface
    #[structopt(name = "ctl")]
    Ctl(CtlCli),
    /// Manage wasmCloud host configuration contexts
    #[structopt(name = "ctx")]
    Ctx(CtxCli),
    /// Manage contents of local wasmCloud caches
    #[structopt(name = "drain")]
    Drain(DrainCli),
    /// Generate code from smithy IDL files
    #[structopt(name = "gen")]
    Gen(GenerateCli),
    /// Utilities for generating and managing keys
    #[structopt(name = "keys", aliases = &["key"])]
    Keys(KeysCli),
    /// Create a new project from template
    #[structopt(name = "new")]
    New(NewCli),
    /// Create, inspect, and modify capability provider archive files
    #[structopt(name = "par")]
    Par(ParCli),
    /// Interact with OCI compliant registries
    #[structopt(name = "reg")]
    Reg(RegCli),
    /// Perform lint checks on smithy models
    #[structopt(name = "lint")]
    Lint(LintCli),
    /// Perform validation checks on smithy models
    #[structopt(name = "validate")]
    Validate(ValidateCli),
}

#[tokio::main]
async fn main() {
    if env_logger::try_init().is_err() {}
    let cli = Cli::from_args();

    let res = match cli.command {
        CliCommand::Call(call_cli) => call::handle_command(call_cli.command()).await,
        CliCommand::Claims(claims_cli) => claims::handle_command(claims_cli.command()).await,
        CliCommand::Ctl(ctl_cli) => ctl::handle_command(ctl_cli.command()).await,
        CliCommand::Ctx(ctx_cli) => ctx::handle_command(ctx_cli.command()).await,
        CliCommand::Drain(drain_cmd) => drain::handle_command(drain_cmd.command()),
        CliCommand::Gen(generate_cli) => smithy::handle_gen_command(generate_cli),
        CliCommand::Keys(keys_cli) => keys::handle_command(keys_cli.command()),
        CliCommand::New(new_cli) => generate::handle_command(new_cli.command()),
        CliCommand::Par(par_cli) => par::handle_command(par_cli.command()).await,
        CliCommand::Reg(reg_cli) => reg::handle_command(reg_cli.command()).await,
        CliCommand::Lint(lint_cli) => smithy::handle_lint_command(lint_cli).await,
        CliCommand::Validate(validate_cli) => smithy::handle_validate_command(validate_cli).await,
    };

    std::process::exit(match res {
        Ok(out) => {
            println!("{}", out);
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    })
}

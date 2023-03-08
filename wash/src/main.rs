use std::collections::HashMap;

use anyhow::Result;
use app::AppCliCommand;
use build::BuildCommand;
use call::CallCli;
use clap::{Parser, Subcommand};
use completions::CompletionOpts;
use ctl::CtlCliCommand;
use ctx::CtxCommand;
use down::DownCommand;
use generate::NewCliCommand;
use keys::KeysCliCommand;
use par::ParCliCommand;
use reg::RegCliCommand;
use serde_json::json;
use smithy::{GenerateCli, LintCli, ValidateCli};
use up::UpCommand;
use wash_lib::cli::claims::ClaimsCliCommand;
use wash_lib::cli::inspect::InspectCliCommand;
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::drain::Drain as DrainSelection;

mod app;
mod appearance;
mod build;
mod call;
mod cfg;
mod completions;
mod ctl;
mod ctx;
mod down;
mod drain;
mod generate;
mod keys;
mod par;
mod reg;
mod smithy;
mod up;
mod util;

const ASCII: &str = r#"
_________________________________________________________________________________
                               _____ _                 _    _____ _          _ _
                              / ____| |               | |  / ____| |        | | |
 __      ____ _ ___ _ __ ___ | |    | | ___  _   _  __| | | (___ | |__   ___| | |
 \ \ /\ / / _` / __| '_ ` _ \| |    | |/ _ \| | | |/ _` |  \___ \| '_ \ / _ \ | |
  \ V  V / (_| \__ \ | | | | | |____| | (_) | |_| | (_| |  ____) | | | |  __/ | |
   \_/\_/ \__,_|___/_| |_| |_|\_____|_|\___/ \__,_|\__,_| |_____/|_| |_|\___|_|_|
_________________________________________________________________________________

A single CLI to handle all of your wasmCloud tooling needs
"#;

#[derive(Debug, Clone, Parser)]
#[clap(name = "wash", about = ASCII, version)]
struct Cli {
    #[clap(
        short = 'o',
        long = "output",
        default_value = "text",
        help = "Specify output format (text or json)",
        global = true
    )]
    pub(crate) output: OutputKind,

    #[clap(subcommand)]
    command: CliCommand,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Subcommand)]
enum CliCommand {
    /// Manage declarative applications and deployments (wadm) (experimental)
    #[clap(name = "app", subcommand)]
    App(AppCliCommand),
    /// Build (and sign) a wasmCloud actor, provider, or interface
    #[clap(name = "build")]
    Build(BuildCommand),
    /// Invoke a wasmCloud actor
    #[clap(name = "call")]
    Call(CallCli),
    /// Generate shell completions
    #[clap(name = "completions")]
    Completions(CompletionOpts),
    /// Generate and manage JWTs for wasmCloud actors
    #[clap(name = "claims", subcommand)]
    Claims(ClaimsCliCommand),
    /// Interact with a wasmCloud control interface
    #[clap(name = "ctl", subcommand)]
    Ctl(CtlCliCommand),
    /// Manage wasmCloud host configuration contexts
    #[clap(name = "ctx", subcommand)]
    Ctx(CtxCommand),
    /// Tear down a wasmCloud environment launched with wash up
    #[clap(name = "down")]
    Down(DownCommand),
    /// Manage contents of local wasmCloud caches
    #[clap(name = "drain", subcommand)]
    Drain(DrainSelection),
    /// Generate code from smithy IDL files
    #[clap(name = "gen")]
    Gen(GenerateCli),
    /// Inspect capability provider or actor module
    #[clap(name = "inspect")]
    Inspect(InspectCliCommand),
    /// Utilities for generating and managing keys
    #[clap(name = "keys", subcommand)]
    Keys(KeysCliCommand),
    /// Perform lint checks on smithy models
    #[clap(name = "lint")]
    Lint(LintCli),
    /// Create a new project from template
    #[clap(name = "new", subcommand)]
    New(NewCliCommand),
    /// Create, inspect, and modify capability provider archive files
    #[clap(name = "par", subcommand)]
    Par(ParCliCommand),
    /// Interact with OCI compliant registries
    #[clap(name = "reg", subcommand)]
    Reg(RegCliCommand),
    /// Bootstrap a wasmCloud environment
    #[clap(name = "up")]
    Up(UpCommand),
    /// Perform validation checks on smithy models
    #[clap(name = "validate")]
    Validate(ValidateCli),
}

#[tokio::main]
async fn main() {
    use clap::CommandFactory;
    if env_logger::try_init().is_err() {}
    let cli: Cli = Parser::parse();

    let output_kind = cli.output;

    let res: Result<CommandOutput> = match cli.command {
        CliCommand::App(app_cli) => app::handle_command(app_cli, output_kind).await,
        CliCommand::Build(build_cli) => build::handle_command(build_cli),
        CliCommand::Call(call_cli) => call::handle_command(call_cli.command()).await,
        CliCommand::Claims(claims_cli) => {
            wash_lib::cli::claims::handle_command(claims_cli, output_kind).await
        }
        CliCommand::Completions(completions_cli) => {
            completions::handle_command(completions_cli, Cli::command())
        }
        CliCommand::Ctl(ctl_cli) => ctl::handle_command(ctl_cli, output_kind).await,
        CliCommand::Ctx(ctx_cli) => ctx::handle_command(ctx_cli).await,
        CliCommand::Down(down_cli) => down::handle_command(down_cli, output_kind).await,
        CliCommand::Drain(drain_cli) => drain::handle_command(drain_cli),
        CliCommand::Gen(generate_cli) => smithy::handle_gen_command(generate_cli),
        CliCommand::Inspect(inspect_cli) => {
            wash_lib::cli::inspect::handle_command(inspect_cli, output_kind).await
        }
        CliCommand::Keys(keys_cli) => keys::handle_command(keys_cli),
        CliCommand::Lint(lint_cli) => smithy::handle_lint_command(lint_cli).await,
        CliCommand::New(new_cli) => generate::handle_command(new_cli).await,
        CliCommand::Par(par_cli) => par::handle_command(par_cli, output_kind).await,
        CliCommand::Reg(reg_cli) => reg::handle_command(reg_cli, output_kind).await,
        CliCommand::Up(up_cli) => up::handle_command(up_cli, output_kind).await,
        CliCommand::Validate(validate_cli) => smithy::handle_validate_command(validate_cli).await,
    };

    std::process::exit(match res {
        Ok(out) => {
            match output_kind {
                OutputKind::Json => {
                    let mut map = out.map;
                    map.insert("success".to_string(), json!(true));
                    println!("\n{}", serde_json::to_string_pretty(&map).unwrap());
                    0
                }
                OutputKind::Text => {
                    println!("\n{}", out.text);
                    // on the first non-error, non-json use of wash, print info about shell completions
                    match completions::first_run_suggestion() {
                        Ok(Some(suggestion)) => {
                            println!("\n{}", suggestion);
                            0
                        }
                        Ok(None) => {
                            // >1st run,  no message
                            0
                        }
                        Err(e) => {
                            // error creating first-run token file
                            eprintln!("\nError: {}", e);
                            1
                        }
                    }
                }
            }
        }
        Err(e) => {
            match output_kind {
                OutputKind::Json => {
                    let mut map = HashMap::new();
                    map.insert("success".to_string(), json!(false));
                    map.insert("error".to_string(), json!(e.to_string()));

                    let error_chain = e
                        .chain()
                        .skip(1)
                        .map(|e| format!("{e}"))
                        .collect::<Vec<String>>();

                    if !error_chain.is_empty() {
                        map.insert("error_chain".to_string(), json!(error_chain));
                    }

                    let backtrace = e.backtrace().to_string();

                    if !backtrace.is_empty() && backtrace != "disabled backtrace" {
                        map.insert("backtrace".to_string(), json!(backtrace));
                    }

                    eprintln!("\n{}", serde_json::to_string_pretty(&map).unwrap());
                }
                OutputKind::Text => {
                    eprintln!("\n{e:?}");
                }
            }
            1
        }
    })
}

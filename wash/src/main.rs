use std::collections::HashMap;

use anyhow::Result;
use call::CallCli;
use claims::ClaimsCliCommand;
use clap::{Parser, Subcommand};
use ctl::CtlCliCommand;
use ctx::CtxCommand;
use drain::DrainSelection;
use generate::NewCliCommand;
use keys::KeysCliCommand;
use par::ParCliCommand;
use reg::RegCliCommand;
use serde_json::json;
use smithy::{GenerateCli, LintCli, ValidateCli};
use util::CommandOutput;

use crate::util::OutputKind;

mod appearance;
mod call;
mod cfg;
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
    /// Invoke a wasmCloud actor
    #[clap(name = "call")]
    Call(CallCli),
    /// Generate and manage JWTs for wasmCloud actors
    #[clap(name = "claims", subcommand)]
    Claims(ClaimsCliCommand),
    /// Interact with a wasmCloud control interface
    #[clap(name = "ctl", subcommand)]
    Ctl(CtlCliCommand),
    /// Manage wasmCloud host configuration contexts
    #[clap(name = "ctx", subcommand)]
    Ctx(CtxCommand),
    /// Manage contents of local wasmCloud caches
    #[clap(name = "drain", subcommand)]
    Drain(DrainSelection),
    /// Generate code from smithy IDL files
    #[clap(name = "gen")]
    Gen(GenerateCli),
    /// Utilities for generating and managing keys
    #[clap(name = "keys", subcommand)]
    Keys(KeysCliCommand),
    /// Create a new project from template
    #[clap(name = "new", subcommand)]
    New(NewCliCommand),
    /// Create, inspect, and modify capability provider archive files
    #[clap(name = "par", subcommand)]
    Par(ParCliCommand),
    /// Interact with OCI compliant registries
    #[clap(name = "reg", subcommand)]
    Reg(RegCliCommand),
    /// Perform lint checks on smithy models
    #[clap(name = "lint")]
    Lint(LintCli),
    /// Perform validation checks on smithy models
    #[clap(name = "validate")]
    Validate(ValidateCli),
}

#[tokio::main]
async fn main() {
    if env_logger::try_init().is_err() {}
    let cli: Cli = Parser::parse();

    let output_kind = cli.output;

    let res: Result<CommandOutput> = match cli.command {
        CliCommand::Call(call_cli) => call::handle_command(call_cli.command()).await,
        CliCommand::Claims(claims_cli) => claims::handle_command(claims_cli, output_kind).await,
        CliCommand::Ctl(ctl_cli) => ctl::handle_command(ctl_cli, output_kind).await,
        CliCommand::Ctx(ctx_cli) => ctx::handle_command(ctx_cli).await,
        CliCommand::Drain(drain_cmd) => drain::handle_command(drain_cmd),
        CliCommand::Gen(generate_cli) => smithy::handle_gen_command(generate_cli),
        CliCommand::Keys(keys_cli) => keys::handle_command(keys_cli),
        CliCommand::New(new_cli) => generate::handle_command(new_cli),
        CliCommand::Par(par_cli) => par::handle_command(par_cli, output_kind).await,
        CliCommand::Reg(reg_cli) => reg::handle_command(reg_cli, output_kind).await,
        CliCommand::Lint(lint_cli) => smithy::handle_lint_command(lint_cli).await,
        CliCommand::Validate(validate_cli) => smithy::handle_validate_command(validate_cli).await,
    };

    std::process::exit(match res {
        Ok(out) => {
            match output_kind {
                OutputKind::Json => {
                    let mut map = out.map;
                    map.insert("success".to_string(), json!(true));
                    println!("\n{}", serde_json::to_string_pretty(&map).unwrap());
                }
                OutputKind::Text => {
                    println!("\n{}", out.text);
                }
            }

            0
        }
        Err(e) => {
            let trace = e
                .chain()
                .skip(1)
                .map(|e| format!("{}", e))
                .collect::<Vec<String>>();

            match output_kind {
                OutputKind::Json => {
                    let mut map = HashMap::new();
                    map.insert("success".to_string(), json!(false));
                    map.insert("error".to_string(), json!(e.to_string()));
                    if !trace.is_empty() {
                        map.insert("trace".to_string(), json!(trace));
                    }

                    eprintln!("\n{}", serde_json::to_string_pretty(&map).unwrap());
                }
                OutputKind::Text => {
                    eprintln!("\n{}", e);
                    if !trace.is_empty() {
                        eprintln!("Error trace:");
                        eprintln!("{}", trace.join("\n"));
                    }
                }
            }
            1
        }
    })
}

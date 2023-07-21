use std::collections::HashMap;

use anyhow::Result;
use dev::DevCommand;
use serde_json::json;
use smithy::{GenerateCli, LintCli, ValidateCli};
use wash_lib::{
    cli::{
        capture::{CaptureCommand, CaptureSubcommand},
        claims::ClaimsCliCommand,
        get::GetCommand,
        inspect::InspectCliCommand,
        link::LinkCommand,
        registry::{RegistryCommand, RegistryPullCommand, RegistryPushCommand},
        spy::SpyCommand,
        start::StartCommand,
        stop::StopCommand,
        CommandOutput, OutputKind,
    },
    drain::Drain as DrainSelection,
};

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
use ui::UiCommand;
use up::UpCommand;

mod app;
mod appearance;
mod build;
mod call;
mod cfg;
mod common;
mod completions;
mod ctl;
mod ctx;
mod dev;
mod down;
mod drain;
mod generate;
mod keys;
mod par;
mod smithy;
mod ui;
mod up;
mod util;

const HELP: &str = r#"
_________________________________________________________________________________
                               _____ _                 _    _____ _          _ _
                              / ____| |               | |  / ____| |        | | |
 __      ____ _ ___ _ __ ___ | |    | | ___  _   _  __| | | (___ | |__   ___| | |
 \ \ /\ / / _` / __| '_ ` _ \| |    | |/ _ \| | | |/ _` |  \___ \| '_ \ / _ \ | |
  \ V  V / (_| \__ \ | | | | | |____| | (_) | |_| | (_| |  ____) | | | |  __/ | |
   \_/\_/ \__,_|___/_| |_| |_|\_____|_|\___/ \__,_|\__,_| |_____/|_| |_|\___|_|_|
_________________________________________________________________________________

Interact and manage wasmCloud applications, projects, and runtime environments

Usage: wash [OPTIONS] <COMMAND>

Build:
  new          Create a new project from template
  build        Build (and sign) a wasmCloud actor, capability provider, or interface
  dev          Run a actor development loop (experimental)
  inspect      Inspect capability provider or actor module
  par          Create, inspect, and modify capability provider archive files

Run:
  up           Bootstrap a local wasmCloud environment
  down         Tear down a local wasmCloud environment (launched with wash up)
  app          Manage declarative applications and deployments (wadm)
  spy          Spy on all invocations between an actor and its linked providers
  ui           Launch a web UI for wasmCloud (experimental)

Iterate:
  get          Get information about different resources
  start        Start an actor or provider
  link         Link an actor and a provider
  call         Invoke a wasmCloud actor
  stop         Stop an actor or provider, or host
  ctl          Interact with a wasmCloud control interface

Publish:
  pull         Pull an artifact from an OCI compliant registry
  push         Push an artifact to an OCI compliant registry
  reg          Perform operations on an OCI or Bindle registry

Configure:
  completions  Generate shell completions for wash
  ctx          Manage wasmCloud host configuration contexts
  drain        Manage contents of local wasmCloud caches
  keys         Utilities for generating and managing keys
  claims       Generate and manage JWTs for wasmCloud actors

Optimize:
  gen          Generate code from smithy IDL files
  lint         Perform lint checks on smithy models
  validate     Perform validation checks on smithy models

Options:
  -o, --output <OUTPUT>  Specify output format (text or json) [default: text]
  --experimental         Whether or not to enable experimental features [default: false]
  -h, --help             Print help
  -V, --version          Print version
"#;

#[derive(Debug, Clone, Parser)]
#[clap(name = "wash", version, override_help = HELP)]
struct Cli {
    #[clap(
        short = 'o',
        long = "output",
        default_value = "text",
        help = "Specify output format (text or json)",
        global = true
    )]
    pub(crate) output: OutputKind,

    #[clap(
        long = "experimental",
        id = "experimental",
        env = "WASH_EXPERIMENTAL",
        default_value = "false",
        help = "Whether or not to enable experimental features",
        global = true
    )]
    pub(crate) experimental: bool,

    #[clap(subcommand)]
    command: CliCommand,
}

// NOTE: If you change the description here, ensure you also change it in the help text constant above
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
    /// Capture and debug cluster invocations and state
    #[clap(name = "capture")]
    Capture(CaptureCommand),
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
    /// (experimental) Run a local development loop for an actor
    #[clap(name = "dev")]
    Dev(DevCommand),
    /// Tear down a wasmCloud environment launched with wash up
    #[clap(name = "down")]
    Down(DownCommand),
    /// Manage contents of local wasmCloud caches
    #[clap(name = "drain", subcommand)]
    Drain(DrainSelection),
    /// Generate code from smithy IDL files
    #[clap(name = "gen")]
    Gen(GenerateCli),
    /// Get information about different resources
    #[clap(name = "get", subcommand)]
    Get(GetCommand),
    /// Inspect capability provider or actor module
    #[clap(name = "inspect")]
    Inspect(InspectCliCommand),
    /// Utilities for generating and managing keys
    #[clap(name = "keys", subcommand)]
    Keys(KeysCliCommand),
    /// Perform lint checks on smithy models
    #[clap(name = "lint")]
    Lint(LintCli),
    /// Link an actor and a provider
    #[clap(name = "link", subcommand)]
    Link(LinkCommand),
    /// Create a new project from template
    #[clap(name = "new", subcommand)]
    New(NewCliCommand),
    /// Create, inspect, and modify capability provider archive files
    #[clap(name = "par", subcommand)]
    Par(ParCliCommand),
    /// Interact with OCI compliant registries
    #[clap(name = "reg", subcommand)]
    Reg(RegistryCommand),
    /// Push an artifact to an OCI compliant registry
    #[clap(name = "push")]
    RegPush(RegistryPushCommand),
    /// Pull an artifact from an OCI compliant registry
    #[clap(name = "pull")]
    RegPull(RegistryPullCommand),
    /// (experimental) Spy on all invocations between an actor and its linked providers
    #[clap(name = "spy")]
    Spy(SpyCommand),
    /// Start an actor or a provider
    #[clap(name = "start", subcommand)]
    Start(StartCommand),
    /// Stop an actor, provider, or host
    #[clap(name = "stop", subcommand)]
    Stop(StopCommand),
    /// Bootstrap a wasmCloud environment
    #[clap(name = "up")]
    Up(UpCommand),
    /// Start a wasmCloud web ui (washboard)
    #[clap(name = "ui")]
    Ui(UiCommand),
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
        CliCommand::Build(build_cli) => build::handle_command(build_cli).await,
        CliCommand::Call(call_cli) => call::handle_command(call_cli.command()).await,
        CliCommand::Capture(capture_cli) => {
            if !cli.experimental {
                experimental_error_message("capture")
            } else if let Some(CaptureSubcommand::Replay(cmd)) = capture_cli.replay {
                wash_lib::cli::capture::handle_replay_command(cmd).await
            } else {
                wash_lib::cli::capture::handle_command(capture_cli).await
            }
        }
        CliCommand::Claims(claims_cli) => {
            wash_lib::cli::claims::handle_command(claims_cli, output_kind).await
        }
        CliCommand::Completions(completions_cli) => {
            completions::handle_command(completions_cli, Cli::command())
        }
        CliCommand::Ctl(ctl_cli) => ctl::handle_command(ctl_cli, output_kind).await,
        CliCommand::Ctx(ctx_cli) => ctx::handle_command(ctx_cli).await,
        CliCommand::Dev(dev_cli) => {
            if cli.experimental {
                dev::handle_command(dev_cli, output_kind).await
            } else {
                experimental_error_message("dev")
            }
        }
        CliCommand::Down(down_cli) => down::handle_command(down_cli, output_kind).await,
        CliCommand::Drain(drain_cli) => drain::handle_command(drain_cli),
        CliCommand::Get(get_cli) => common::get_cmd::handle_command(get_cli, output_kind).await,
        CliCommand::Gen(generate_cli) => smithy::handle_gen_command(generate_cli),
        CliCommand::Inspect(inspect_cli) => {
            wash_lib::cli::inspect::handle_command(inspect_cli, output_kind).await
        }
        CliCommand::Keys(keys_cli) => keys::handle_command(keys_cli),
        CliCommand::Lint(lint_cli) => smithy::handle_lint_command(lint_cli).await,
        CliCommand::Link(link_cli) => common::link_cmd::handle_command(link_cli, output_kind).await,
        CliCommand::New(new_cli) => generate::handle_command(new_cli).await,
        CliCommand::Par(par_cli) => par::handle_command(par_cli, output_kind).await,
        CliCommand::Reg(reg_cli) => {
            common::registry_cmd::handle_command(reg_cli, output_kind).await
        }
        CliCommand::RegPush(reg_push_cli) => {
            common::registry_cmd::registry_push(reg_push_cli, output_kind).await
        }
        CliCommand::RegPull(reg_pull_cli) => {
            common::registry_cmd::registry_pull(reg_pull_cli, output_kind).await
        }
        CliCommand::Spy(spy_cli) => {
            if !cli.experimental {
                experimental_error_message("spy")
            } else {
                wash_lib::cli::spy::handle_command(spy_cli).await
            }
        }
        CliCommand::Start(start_cli) => {
            common::start_cmd::handle_command(start_cli, output_kind).await
        }
        CliCommand::Stop(stop_cli) => common::stop_cmd::handle_command(stop_cli, output_kind).await,
        CliCommand::Up(up_cli) => up::handle_command(up_cli, output_kind).await,
        CliCommand::Ui(ui_cli) => {
            if cli.experimental {
                ui::handle_command(ui_cli, output_kind).await
            } else {
                experimental_error_message("ui")
            }
        }
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

fn experimental_error_message(command: &str) -> Result<CommandOutput> {
    Err(anyhow::anyhow!("The `wash {command}` command is experimental and may change in future releases. Set the `WASH_EXPERIMENTAL` environment variable or `--experimental` flag to `true` to use this command."))
}

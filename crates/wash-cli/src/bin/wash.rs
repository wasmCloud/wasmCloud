use std::collections::HashMap;

use anyhow::bail;
use clap::{self, Parser, Subcommand};
use serde_json::json;
use tracing_subscriber::EnvFilter;
use wash_cli::app::{self, AppCliCommand};
use wash_cli::build::{self, BuildCommand};
use wash_cli::call::{self, CallCli};
use wash_cli::common;
use wash_cli::completions::{self, CompletionOpts};
use wash_cli::config::{self, ConfigCliCommand};
use wash_cli::ctx::{self, CtxCommand};
use wash_cli::dev::{self, DevCommand};
use wash_cli::down::{self, DownCommand};
use wash_cli::drain;
use wash_cli::generate::{self, NewCliCommand};
use wash_cli::keys::{self, KeysCliCommand};
use wash_cli::par::{self, ParCliCommand};
use wash_cli::ui::{self, UiCommand};
use wash_cli::up::{self, UpCommand};
use wash_lib::cli::capture::{CaptureCommand, CaptureSubcommand};
use wash_lib::cli::claims::ClaimsCliCommand;
use wash_lib::cli::get::GetCommand;
use wash_lib::cli::inspect::InspectCliCommand;
use wash_lib::cli::label::LabelHostCommand;
use wash_lib::cli::link::LinkCommand;
use wash_lib::cli::registry::{RegistryPullCommand, RegistryPushCommand};
use wash_lib::cli::scale::ScaleCommand;
use wash_lib::cli::spy::SpyCommand;
use wash_lib::cli::start::StartCommand;
use wash_lib::cli::stop::StopCommand;
use wash_lib::cli::update::UpdateCommand;
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::drain::Drain as DrainSelection;

const HELP: &str = r"
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
  new          Create a new project from a template
  build        Build (and sign) a wasmCloud component or capability provider
  dev          Start a developer loop to hot-reload a local wasmCloud component
  inspect      Inspect a capability provider or Wasm component for signing information and interfaces
  par          Create, inspect, and modify capability provider archive files

Run:
  up           Bootstrap a local wasmCloud environment
  down         Tear down a local wasmCloud environment (launched with wash up)
  app          Manage declarative applications and deployments (wadm)
  spy          Spy on all invocations a component sends and receives
  ui           Serve a web UI for wasmCloud

Iterate:
  get          Get information about different running wasmCloud resources
  start        Start a component or capability provider
  scale        Scale a component running in a host to a certain level of concurrency
  stop         Stop a component, capability provider, or host
  update       Update a component running in a host to newer image reference
  link         Link one component to another on a set of interfaces
  call         Invoke a simple function on a component running in a wasmCloud host
  label        Label (or un-label) a host with a key=value label pair
  config       Create configuration for components, capability providers and links

Publish:
  pull         Pull an artifact from an OCI compliant registry
  push         Push an artifact to an OCI compliant registry
  reg          Perform operations on an OCI registry

Configure:
  completions  Generate shell completions for wash
  ctx          Manage wasmCloud host configuration contexts
  drain        Manage contents of local wasmCloud caches
  keys         Utilities for generating and managing signing keys
  claims       Generate and manage JWTs for wasmCloud components and capability providers

Options:
  -o, --output <OUTPUT>  Specify output format (text or json) [default: text]
  --experimental         Whether or not to enable experimental features [default: false]
  -h, --help             Print help
  -V, --version          Print version
";

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
    /// Manage declarative applications and deployments (wadm)
    #[clap(name = "app", subcommand)]
    App(AppCliCommand),
    /// Build (and sign) a wasmCloud component or capability provider
    #[clap(name = "build")]
    Build(BuildCommand),
    /// Invoke a simple function on a component running in a wasmCloud host
    #[clap(name = "call")]
    Call(CallCli),
    /// Capture and debug cluster invocations and state
    #[clap(name = "capture")]
    Capture(CaptureCommand),
    /// Generate shell completions
    #[clap(name = "completions")]
    Completions(CompletionOpts),
    /// Generate and manage JWTs for wasmCloud components and capability providers
    #[clap(name = "claims", subcommand)]
    Claims(ClaimsCliCommand),
    /// Create configuration for components, capability providers and links
    #[clap(name = "config", subcommand)]
    Config(ConfigCliCommand),
    /// Manage wasmCloud host configuration contexts
    #[clap(name = "ctx", alias = "context", alias = "contexts", subcommand)]
    Ctx(CtxCommand),
    /// Start a developer loop to hot-reload a local wasmCloud component
    #[clap(name = "dev")]
    Dev(DevCommand),
    /// Tear down a wasmCloud environment launched with wash up
    #[clap(name = "down")]
    Down(DownCommand),
    /// Manage contents of local wasmCloud caches
    #[clap(name = "drain", subcommand)]
    Drain(DrainSelection),
    /// Get information about different running wasmCloud resources
    #[clap(name = "get", subcommand)]
    Get(GetCommand),
    /// Inspect a capability provider or Wasm component for signing information and interfaces
    #[clap(name = "inspect")]
    Inspect(InspectCliCommand),
    /// Utilities for generating and managing signing keys
    #[clap(name = "keys", alias = "key", subcommand)]
    Keys(KeysCliCommand),
    /// Link one component to another on a set of interfaces
    #[clap(name = "link", alias = "links", subcommand)]
    Link(LinkCommand),
    /// Create a new project from a template
    #[clap(name = "new", subcommand)]
    New(NewCliCommand),
    /// Create, inspect, and modify capability provider archive files
    #[clap(name = "par", subcommand)]
    Par(ParCliCommand),
    /// Push an artifact to an OCI compliant registry
    #[clap(name = "push")]
    RegPush(RegistryPushCommand),
    /// Pull an artifact from an OCI compliant registry
    #[clap(name = "pull")]
    RegPull(RegistryPullCommand),
    /// Spy on all invocations a component sends and receives
    #[clap(name = "spy")]
    Spy(SpyCommand),
    /// Scale a component running in a host to a certain level of concurrency
    #[clap(name = "scale", subcommand)]
    Scale(ScaleCommand),
    /// Start a component or capability provider
    #[clap(name = "start", subcommand)]
    Start(StartCommand),
    /// Stop a component, capability provider, or host
    #[clap(name = "stop", subcommand)]
    Stop(StopCommand),
    /// Label (or un-label) a host with a key=value label pair
    #[clap(name = "label", alias = "tag")]
    Label(LabelHostCommand),
    /// Update a component running in a host to newer image reference
    #[clap(name = "update", subcommand)]
    Update(UpdateCommand),
    /// Bootstrap a wasmCloud environment
    #[clap(name = "up")]
    Up(UpCommand),
    /// Serve a web UI for wasmCloud
    #[clap(name = "ui")]
    Ui(UiCommand),
}

#[tokio::main]
async fn main() {
    use clap::CommandFactory;
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli: Cli = Parser::parse();

    let output_kind = cli.output;

    // Whether or not to append `success: true` to the output JSON. For now, we only omit it for `wash config get`.
    let append_json_success = !matches!(
        cli.command,
        CliCommand::Config(ConfigCliCommand::GetCommand { .. }),
    );
    let res: anyhow::Result<CommandOutput> = match cli.command {
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
        CliCommand::Config(config_cli) => config::handle_command(config_cli, output_kind).await,
        CliCommand::Ctx(ctx_cli) => ctx::handle_command(ctx_cli).await,
        CliCommand::Dev(dev_cli) => dev::handle_command(dev_cli, output_kind).await,
        CliCommand::Down(down_cli) => down::handle_command(down_cli, output_kind).await,
        CliCommand::Drain(drain_cli) => drain::handle_command(drain_cli),
        CliCommand::Get(get_cli) => common::get_cmd::handle_command(get_cli, output_kind).await,
        CliCommand::Inspect(inspect_cli) => {
            wash_lib::cli::inspect::handle_command(inspect_cli, output_kind).await
        }
        CliCommand::Keys(keys_cli) => keys::handle_command(keys_cli),
        CliCommand::Link(link_cli) => common::link_cmd::handle_command(link_cli, output_kind).await,
        CliCommand::New(new_cli) => generate::handle_command(new_cli).await,
        CliCommand::Par(par_cli) => par::handle_command(par_cli, output_kind).await,
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
        CliCommand::Scale(scale_cli) => {
            common::scale_cmd::handle_command(scale_cli, output_kind).await
        }
        CliCommand::Start(start_cli) => {
            common::start_cmd::handle_command(start_cli, output_kind).await
        }
        CliCommand::Stop(stop_cli) => common::stop_cmd::handle_command(stop_cli, output_kind).await,
        CliCommand::Label(label_cli) => {
            common::label_cmd::handle_command(label_cli, output_kind).await
        }
        CliCommand::Update(update_cli) => {
            common::update_cmd::handle_command(update_cli, output_kind).await
        }
        CliCommand::Up(up_cli) => up::handle_command(up_cli, output_kind).await,
        CliCommand::Ui(ui_cli) => ui::handle_command(ui_cli, output_kind).await,
    };

    std::process::exit(match res {
        Ok(out) => {
            match output_kind {
                OutputKind::Json => {
                    let mut map = out.map;
                    // When we fetch configuration, we don't want to arbitrarily insert a key into the map.
                    // There may be other commands we do this in the future, but for now the special check is fine.
                    if append_json_success {
                        map.insert("success".to_string(), json!(true));
                    }
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

fn experimental_error_message(command: &str) -> anyhow::Result<CommandOutput> {
    bail!("The `wash {command}` command is experimental and may change in future releases. Set the `WASH_EXPERIMENTAL` environment variable or `--experimental` flag to `true` to use this command.")
}

use std::env;
use std::fmt::{Display, Formatter};

use anyhow::anyhow;
use clap::{self, FromArgMatches, Parser, Subcommand};
use console::style;
use crossterm::style::Stylize;
use crate::lib::cli::CliConnectionOpts;
use crate::lib::cli::capture::CaptureCommand;
use crate::lib::cli::claims::ClaimsCliCommand;
use crate::lib::cli::get::GetCommand;
use crate::lib::cli::inspect::InspectCliCommand;
use crate::lib::cli::label::LabelHostCommand;
use crate::lib::cli::link::LinkCommand;
use crate::lib::cli::registry::{RegistryPullCommand, RegistryPushCommand};
use crate::lib::cli::scale::ScaleCommand;
use crate::lib::cli::spy::SpyCommand;
use crate::lib::cli::start::StartCommand;
use crate::lib::cli::stop::StopCommand;
use crate::lib::cli::update::UpdateCommand;
use crate::lib::cli::OutputKind;
use crate::lib::drain::Drain as DrainSelection;

use crate::app::AppCliCommand;
use crate::build::BuildCommand;
use crate::call::CallCli;
use crate::cmd::config::ConfigCliCommand;
use crate::cmd::dev::DevCommand;
use crate::cmd::up::UpCommand;
use crate::cmd::wit::WitCommand;
use crate::completions::CompletionOpts;
use crate::ctx::CtxCommand;
use crate::down::DownCommand;
use crate::generate::NewCliCommand;
use crate::keys::KeysCliCommand;
use crate::par::ParCliCommand;
use crate::plugin::PluginCommand;
use crate::secrets::SecretsCliCommand;
use crate::style::WASH_CLI_STYLE;
use crate::ui::UiCommand;

#[derive(Clone)]
struct HelpTopic {
    name: &'static str,
    commands: Vec<(&'static str, &'static str)>,
}

impl Display for HelpTopic {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        const PADDING_AFTER_LONGEST_SPACES: usize = 3;
        const DEFAULT_PADDING_START: usize = 25;
        writeln!(f, "{}", self.name.green().bold())?;
        let longest_command_length = self
            .commands
            .iter()
            .map(|(name, _)| name.len())
            .max()
            .unwrap_or(DEFAULT_PADDING_START)
            + PADDING_AFTER_LONGEST_SPACES;

        for (name, desc) in &self.commands {
            let padding = " ".repeat(longest_command_length - name.len());
            writeln!(f, "  {}{}{}", name.blue(), padding, desc)?;
        }
        Ok(())
    }
}

struct HelpTopics(Vec<HelpTopic>);

impl Display for HelpTopics {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for topic in &self.0 {
            writeln!(f, "{topic}")?;
        }
        Ok(())
    }
}

impl From<Vec<HelpTopic>> for HelpTopics {
    fn from(topics: Vec<HelpTopic>) -> Self {
        HelpTopics(topics)
    }
}

fn create_colored_help() -> String {
    let banner = style(
        r"
_________________________________________________________________________________
                               _____ _                 _    _____ _          _ _
                              / ____| |               | |  / ____| |        | | |
 __      ____ _ ___ _ __ ___ | |    | | ___  _   _  __| | | (___ | |__   ___| | |
 \ \ /\ / / _` / __| '_ ` _ \| |    | |/ _ \| | | |/ _` |  \___ \| '_ \ / _ \ | |
  \ V  V / (_| \__ \ | | | | | |____| | (_) | |_| | (_| |  ____) | | | |  __/ | |
   \_/\_/ \__,_|___/_| |_| |_|\_____|_|\___/ \__,_|\__,_| |_____/|_| |_|\___|_|_|
_________________________________________________________________________________
",
    )
    .green()
    .bold();

    let description =
        "Interact and manage wasmCloud applications, projects, and runtime environments".green();

    let usage_description = format!("{} {}", "Usage:".green(), "[OPTIONS] <COMMAND>".blue());

    let command_descriptions = HelpTopics::from([
        HelpTopic {
            name: "Build:",
            commands: vec![
                ("new", "Create a new project from a template or git repository"),
                ("build", "Build (and sign) a wasmCloud component or capability provider"),
                ("dev", "Start a developer loop to hot-reload a local wasmCloud component"),
                (
                    "inspect",
                    "Inspect a Wasm component or capability provider for signing information and interfaces",
                ),
                (
                    "par",
                    "Create, inspect, and modify capability provider archive files",
                ),
                (
                    "wit",
                    "Create wit packages and fetch wit dependencies for a component",
                ),
            ],
        },
        HelpTopic {
            name: "Run:",
            commands: vec![
                ("up", "Bootstrap a local wasmCloud environment"),
                (
                    "down",
                    "Tear down a local wasmCloud environment (launched with wash up)",
                ),
                ("app", "Manage declarative applications and deployments (wadm)"),
                ("spy", "Spy on all invocations a component sends and receives"),
                ("ui", "Serve a web UI for wasmCloud"),
            ],
        },
        HelpTopic {
            name: "Iterate:",
            commands: vec![
                ("get", "Get information about different running wasmCloud resources"),
                ("start", "Start a component or capability provider"),
                (
                    "scale",
                    "Scale a component running in a host to a certain level of concurrency",
                ),
                ("stop", "Stop a component, capability provider, or host"),
                (
                    "update",
                    "Update a component running in a host to newer image reference",
                ),
                ("link", "Link one component to another on a set of interfaces"),
                ("call", "Invoke a simple function on a component running in a wasmCloud host"),
                ("label", "Label (or un-label) a host with a key=value label pair"),
                (
                    "config",
                    "Create configuration for components, capability providers and links",
                ),
                (
                    "secrets",
                    "Create secret references for components, capability providers and links",
                ),
            ],
        },
        HelpTopic {
            name: "Publish:",
            commands: vec![
                ("pull", "Pull an artifact from an OCI compliant registry"),
                ("push", "Push an artifact to an OCI compliant registry"),
            ],
        },
        HelpTopic {
            name: "Configure:",
            commands: vec![
                ("completions", "Generate shell completions for wash"),
                ("ctx", "Manage wasmCloud host configuration contexts"),
                ("drain", "Manage contents of local wasmCloud caches"),
                ("keys", "Generate and manage signing keys"),
                ("claims", "Generate and manage JWTs for wasmCloud components and capability providers"),
                ("plugin", "Manage wash plugins"),
            ],
        },
        HelpTopic {
            name: "Options:",
            commands: vec![
                (
                    "-o, --output <OUTPUT>",
                    "Specify output format (text or json) [default: text]",
                ),
                (
                    "--experimental",
                    "Whether or not to enable experimental features [default: false]",
                ),
                ("-h, --help", "Print help"),
                ("-V, --version", "Print version"),
            ],
        },
    ].to_vec());

    format!(
        r#"
{banner}

{description}

{usage_description}

{command_descriptions}
"#
    )
}

#[derive(Debug, Clone, Parser)]
#[clap(name = "wash", disable_version_flag = true, override_help = create_colored_help())]
#[command(styles = WASH_CLI_STYLE)]
pub struct Cli {
    #[clap(
        short = 'o',
        long = "output",
        default_value = "text",
        help = "Specify output format (text or json)",
        global = true
    )]
    pub output: OutputKind,

    #[clap(
        long = "experimental",
        id = "experimental",
        env = "WASH_EXPERIMENTAL",
        default_value = "false",
        help = "Whether or not to enable experimental features",
        global = true
    )]
    pub experimental: bool,

    #[clap(
        long = "help-markdown",
        conflicts_with = "help",
        hide = true,
        global = true
    )]
    pub help_markdown: bool,

    #[clap(short = 'V', long = "version", help = "Print version")]
    pub version: bool,

    #[clap(subcommand)]
    pub command: Option<CliCommand>,
}

// NOTE: If you change the description here, ensure you also change it in the help text constant above
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Subcommand)]
pub enum CliCommand {
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
    /// Generate shell completions for wash
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
    /// Tear down a local wasmCloud environment (launched with wash up)
    #[clap(name = "down")]
    Down(DownCommand),
    /// Manage contents of local wasmCloud caches
    #[clap(name = "drain", subcommand)]
    Drain(DrainSelection),
    /// Get information about different running wasmCloud resources
    #[clap(name = "get", subcommand)]
    Get(GetCommand),
    /// Inspect a Wasm component or capability provider for signing information and interfaces
    #[clap(name = "inspect")]
    Inspect(InspectCliCommand),
    /// Generate and manage signing keys
    #[clap(name = "keys", alias = "key", subcommand)]
    Keys(KeysCliCommand),
    /// Link one component to another on a set of interfaces
    #[clap(name = "link", alias = "links", subcommand)]
    Link(LinkCommand),
    /// Create a new project from a template or git repository
    #[clap(name = "new", subcommand)]
    New(NewCliCommand),
    /// Create, inspect, and modify capability provider archive files
    #[clap(name = "par", subcommand)]
    Par(ParCliCommand),
    /// Manage wash plugins
    #[clap(name = "plugin", subcommand)]
    Plugin(PluginCommand),
    /// Push an artifact to an OCI compliant registry
    #[clap(name = "push")]
    RegPush(RegistryPushCommand),
    /// Pull an artifact from an OCI compliant registry
    #[clap(name = "pull")]
    RegPull(RegistryPullCommand),
    /// Create secret references for components, capability providers and links
    #[clap(name = "secrets", alias = "secret", subcommand)]
    Secrets(SecretsCliCommand),
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
    /// Bootstrap a local wasmCloud environment
    #[clap(name = "up")]
    Up(UpCommand),
    /// Serve a web UI for wasmCloud
    #[clap(name = "ui")]
    Ui(UiCommand),
    /// Create wit packages and fetch wit dependencies for a component
    #[clap(name = "wit", subcommand)]
    Wit(WitCommand),
}

// Helper function for dynamic autocompletion to create the CliCommand for the non-executed CLI command.
pub(crate) fn create_cli() -> Option<CliCommand> {
    use clap::CommandFactory;
    let mut args: Vec<String> = env::args().collect();
    args.drain(0..2);
    let matches = Cli::command().get_matches_from(args);
    Cli::from_arg_matches(&matches).unwrap().command
}

// Used to identify the connection details for retrieving candidates for dynamic autocompletion
// of the respective subcommand.
// The used clap_complete::ArgValueCompleter has no information about which subcommand requires candidates.
// Only the current string for the respective argument can be passed as an input parameter.
// The subcommand must therefore be recreated to know about its details, e.g. the connection options.
pub(crate) fn get_connection_opts_from_cli() -> anyhow::Result<CliConnectionOpts> {
    match create_cli() {
        Some(CliCommand::App(AppCliCommand::Delete(cmd))) => Ok(cmd.opts),
        Some(CliCommand::App(AppCliCommand::Deploy(cmd))) => Ok(cmd.opts),
        Some(CliCommand::App(AppCliCommand::History(cmd))) => Ok(cmd.opts),
        Some(CliCommand::App(AppCliCommand::Get(cmd))) => Ok(cmd.opts),
        Some(CliCommand::App(AppCliCommand::Put(cmd))) => Ok(cmd.opts),
        Some(CliCommand::App(AppCliCommand::Status(cmd))) => Ok(cmd.opts),
        Some(CliCommand::App(AppCliCommand::Undeploy(cmd))) => Ok(cmd.opts),
        Some(CliCommand::Spy(cmd)) => Ok(cmd.opts),
        _ => Err(anyhow!("Command did not match any expected patterns")),
    }
}

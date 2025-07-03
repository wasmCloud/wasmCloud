use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::io::{stdout, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use clap::{self, Arg, ArgMatches, Command, FromArgMatches, Parser, Subcommand};
use console::style;
use crossterm::style::Stylize;
use etcetera::AppStrategy;
use semver::Version;
use serde_json::json;
use tracing_subscriber::EnvFilter;
use wash::lib::cli::capture::{CaptureCommand, CaptureSubcommand};
use wash::lib::cli::claims::ClaimsCliCommand;
use wash::lib::cli::get::GetCommand;
use wash::lib::cli::inspect::InspectCliCommand;
use wash::lib::cli::label::LabelHostCommand;
use wash::lib::cli::link::LinkCommand;
use wash::lib::cli::registry::{RegistryPullCommand, RegistryPushCommand};
use wash::lib::cli::scale::ScaleCommand;
use wash::lib::cli::spy::SpyCommand;
use wash::lib::cli::start::StartCommand;
use wash::lib::cli::stop::StopCommand;
use wash::lib::cli::update::UpdateCommand;
use wash::lib::cli::{CommandOutput, OutputKind};
use wash::lib::config::WASH_DIRECTORIES;
use wash::lib::drain::Drain as DrainSelection;
use wash::lib::generate::emoji;
use wash::lib::plugin::subcommand::{DirMapping, SubcommandRunner};
use wash::lib::start::get_wash_versions_newer_than;

use wash::cli::app::{self, AppCliCommand};
use wash::cli::build::{self, BuildCommand};
use wash::cli::call::{self, CallCli};
use wash::cli::cmd::config::{self, ConfigCliCommand};
use wash::cli::cmd::dev::{self, DevCommand};
use wash::cli::cmd::link;
use wash::cli::cmd::up::{self, UpCommand};
use wash::cli::cmd::wit::{self, WitCommand};
use wash::cli::common;
use wash::cli::completions::{self, CompletionOpts};
use wash::cli::config::{NATS_SERVER_VERSION, WADM_VERSION, WASMCLOUD_HOST_VERSION};
use wash::cli::ctx::{self, CtxCommand};
use wash::cli::down::{self, DownCommand};
use wash::cli::drain;
use wash::cli::generate::{self, NewCliCommand};
use wash::cli::keys::{self, KeysCliCommand};
use wash::cli::par::{self, ParCliCommand};
use wash::cli::plugin::{self, PluginCommand};
use wash::cli::secrets::{self, SecretsCliCommand};
use wash::cli::style::WASH_CLI_STYLE;
use wash::cli::ui::{self, UiCommand};

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
            writeln!(f, "{}", topic)?;
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

/// Helper function to display the version of all the binaries wash runs
fn version(output: OutputKind) -> String {
    match output {
        OutputKind::Text => format!(
            "wash          v{}\n├ nats-server {}\n├ wadm        {}\n└ wasmcloud   {}",
            clap::crate_version!(),
            NATS_SERVER_VERSION,
            WADM_VERSION,
            WASMCLOUD_HOST_VERSION
        ),
        OutputKind::Json => {
            let versions = serde_json::json!({
                "wash": format!("v{}", clap::crate_version!()),
                "nats-server": NATS_SERVER_VERSION,
                "wadm": WADM_VERSION,
                "wasmcloud": WASMCLOUD_HOST_VERSION,
            });
            serde_json::to_string_pretty(&versions).unwrap()
        }
    }
}

#[derive(Debug, Clone, Parser)]
#[clap(name = "wash", disable_version_flag = true, override_help = create_colored_help())]
#[command(styles = WASH_CLI_STYLE)]
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

    #[clap(
        long = "help-markdown",
        conflicts_with = "help",
        hide = true,
        global = true
    )]
    help_markdown: bool,

    #[clap(short = 'V', long = "version", help = "Print version")]
    version: bool,

    #[clap(subcommand)]
    command: Option<CliCommand>,
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

#[tokio::main]
async fn main() {
    use clap::CommandFactory;
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let mut command = Cli::command();

    // Load plugins if they are not disabled
    let mut resolved_plugins = None;
    if std::env::var("WASH_DISABLE_PLUGINS").is_err() {
        if let Some((plugins, dir)) = load_plugins().await {
            let mut plugin_paths = HashMap::new();
            for plugin in plugins.all_metadata().into_iter().cloned() {
                if command.find_subcommand(&plugin.id).is_some() {
                    tracing::error!(
                        id = %plugin.id,
                        "Plugin ID matches an existing subcommand, skipping",
                    );
                    continue;
                }
                let (flag_args, path_ids): (Vec<_>, Vec<_>) = plugin
                    .flags
                    .into_iter()
                    .map(|(flag, arg)| {
                        let trimmed = flag.trim_start_matches('-').to_owned();
                        // Return a list of args with an Option containing the ID of the flag if it was a path
                        (
                            Arg::new(trimmed.clone())
                                .long(trimmed.clone())
                                .help(arg.description)
                                .required(arg.required),
                            arg.is_path.then_some(trimmed),
                        )
                    })
                    .unzip();
                let (positional_args, positional_ids): (Vec<_>, Vec<_>) = plugin
                    .arguments
                    .into_iter()
                    .map(|(name, arg)| {
                        let trimmed = name.trim().to_owned();
                        // Return a list of args with an Option containing the ID of the argument if it was a path
                        (
                            Arg::new(trimmed.clone())
                                .help(arg.description)
                                .required(arg.required),
                            arg.is_path.then_some(trimmed),
                        )
                    })
                    .unzip();
                let subcmd = Command::new(plugin.id.clone())
                    .about(plugin.description)
                    .author(plugin.author)
                    .version(plugin.version)
                    .display_name(plugin.name)
                    .args(flag_args.into_iter().chain(positional_args));
                command = command.subcommand(subcmd);
                plugin_paths.insert(
                    plugin.id,
                    path_ids
                        .into_iter()
                        .chain(positional_ids)
                        .flatten()
                        .collect::<Vec<_>>(),
                );
            }
            resolved_plugins = Some((plugins, dir, plugin_paths));
        }
    }

    command.build();
    let mut matches = command.get_matches_mut();

    let cli = match (Cli::from_arg_matches(&matches), resolved_plugins) {
        // Received a valid CLI command with no parsed known subcommand, but with a matched subcommand
        // (this is usually a plugin call)
        (Ok(Cli { command: None, .. }), Some((mut plugins, plugin_dir, plugin_paths)))
            if matches.subcommand().is_some() =>
        {
            run_plugin(&mut matches, None, &mut plugins, &plugin_dir, plugin_paths).await
        }
        // Received a valid CLI command
        (Ok(cli), _) => cli,
        // Failed to parse the CLI command, which *may* be a plugin execution
        (Err(mut e), Some((mut plugins, plugin_dir, plugin_paths))) => {
            run_plugin(
                &mut matches,
                Some(&mut e),
                &mut plugins,
                &plugin_dir,
                plugin_paths,
            )
            .await
        }
        // Failed to parse the CLI command, with no possible plugins to call
        (Err(e), None) => {
            e.exit();
        }
    };

    // print info on new wash version if available
    if cli.experimental {
        if let Err(e) = inform_new_wash_version().await {
            eprintln!("Error while checking for new wash version: {e}");
            std::process::exit(2);
        }
    }

    let old_config_dir = WASH_DIRECTORIES.home_dir().join(".wash");
    if tokio::fs::try_exists(&old_config_dir)
        .await
        .unwrap_or(false)
    {
        eprintln!(
            "Old configuration directory '{}' found, consider migrating to new configuration structure.",
            old_config_dir.display(),
        );
    }

    let output_kind = cli.output;

    // Implements clap_markdown for markdown generation of command line documentation. Most straightforward way to invoke is probably `wash app get --help-markdown > help.md`
    if cli.help_markdown {
        clap_markdown::print_help_markdown::<Cli>();
        std::process::exit(0);
    };

    if cli.version {
        println!("{}", version(cli.output));
        std::process::exit(0);
    }

    let cli_command = cli.command.unwrap_or_else(|| {
        eprintln!("{}", command.render_help());
        std::process::exit(2);
    });

    // Whether or not to append `success: true` to the output JSON. For now, we only omit it for `wash config get`.
    let append_json_success = !matches!(
        cli_command,
        CliCommand::Config(ConfigCliCommand::GetCommand { .. }),
    );
    let res: anyhow::Result<CommandOutput> = match cli_command {
        CliCommand::App(app_cli) => app::handle_command(app_cli, output_kind).await,
        CliCommand::Build(build_cli) => build::handle_command(build_cli).await,
        CliCommand::Call(call_cli) => call::handle_command(call_cli.command()).await,
        CliCommand::Capture(capture_cli) => {
            if !cli.experimental {
                experimental_error_message("capture")
            } else if let Some(CaptureSubcommand::Replay(cmd)) = capture_cli.replay {
                wash::lib::cli::capture::handle_replay_command(cmd).await
            } else {
                wash::lib::cli::capture::handle_command(capture_cli).await
            }
        }
        CliCommand::Claims(claims_cli) => {
            wash::lib::cli::claims::handle_command(claims_cli, output_kind).await
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
            wash::lib::cli::inspect::handle_command(inspect_cli, output_kind).await
        }
        CliCommand::Keys(keys_cli) => keys::handle_command(keys_cli),
        CliCommand::Link(link_cli) => link::invoke(link_cli, output_kind).await,
        CliCommand::New(new_cli) => generate::handle_command(new_cli).await,
        CliCommand::Par(par_cli) => par::handle_command(par_cli, output_kind).await,
        CliCommand::Plugin(plugin_cli) => plugin::handle_command(plugin_cli, output_kind).await,
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
                wash::lib::cli::spy::handle_command(spy_cli).await
            }
        }
        CliCommand::Scale(scale_cli) => {
            common::scale_cmd::handle_command(scale_cli, output_kind).await
        }
        CliCommand::Secrets(secrets_cli) => secrets::handle_command(secrets_cli, output_kind).await,
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
        CliCommand::Wit(wit_cli) => wit::handle_command(wit_cli).await,
    };

    // Use buffered writes to stdout preventing a broken pipe error in case this program has been
    // piped to another program (e.g. 'wash dev | jq') and CTRL^C has been pressed.
    let mut stdout_buf = BufWriter::new(stdout().lock());

    let exit_code: i32 = match res {
        Ok(out) => {
            match output_kind {
                OutputKind::Json => {
                    let mut map = out.map;
                    // When we fetch configuration, we don't want to arbitrarily insert a key into the map.
                    // There may be other commands we do this in the future, but for now the special check is fine.
                    if append_json_success {
                        map.insert("success".to_string(), json!(true));
                    }
                    let _ = writeln!(
                        stdout_buf,
                        "\n{}",
                        serde_json::to_string_pretty(&map).unwrap()
                    );
                    0
                }
                OutputKind::Text => {
                    let _ = writeln!(stdout_buf, "\n{}", out.text);
                    // on the first non-error, non-json use of wash, print info about shell completions
                    match completions::first_run_suggestion() {
                        Ok(Some(suggestion)) => {
                            let _ = writeln!(stdout_buf, "\n{suggestion}");
                            0
                        }
                        Ok(None) => {
                            // >1st run,  no message
                            0
                        }
                        Err(e) => {
                            // error creating first-run token file
                            eprintln!("\nError: {e}");
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
    };

    let _ = stdout_buf.flush();
    std::process::exit(exit_code);
}

/// Run a plugin
async fn run_plugin(
    matches: &mut ArgMatches,
    maybe_error: Option<&mut clap::Error>,
    plugins: &mut SubcommandRunner,
    plugin_dir: &PathBuf,
    plugin_paths: HashMap<String, Vec<String>>,
) -> ! {
    let (id, sub_matches) = match matches.subcommand() {
        Some(data) => data,
        None => {
            if let Some(e) = maybe_error {
                e.exit();
            } else {
                clap::Error::raw(
                    clap::error::ErrorKind::InvalidSubcommand,
                    "failed to find named plugin",
                )
                .exit();
            }
        }
    };

    let dir = match ensure_plugin_scratch_dir_exists(plugin_dir, id).await {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("Error creating plugin scratch directory: {}", e);
            std::process::exit(1);
        }
    };

    let mut plugin_dirs = Vec::new();

    // Try fetching all path matches from args marked as paths
    plugin_dirs.extend(
        plugin_paths
            .get(id)
            .map(ToOwned::to_owned)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| {
                sub_matches.get_one::<String>(&id).map(|path| DirMapping {
                    host_path: path.into(),
                    component_path: None,
                })
            }),
    );

    if plugins.metadata(id).is_none() {
        if let Some(e) = maybe_error {
            e.insert(
                clap::error::ContextKind::InvalidSubcommand,
                clap::error::ContextValue::String("No plugin found for subcommand".to_string()),
            );
            e.exit();
        } else {
            clap::Error::raw(
                clap::error::ErrorKind::InvalidSubcommand,
                "No plugin found for subcommand",
            )
            .exit();
        }
    }

    // NOTE(thomastaylor312): This is a hack to get the raw args to pass to the plugin. I
    // don't really love this, but we can't add nice help text and structured arguments to
    // the CLI if we just get the raw args with `allow_external_subcommands(true)`. We can
    // revisit this later with something if we need to. I did do some basic testing that
    // even if you wrap wash in a shell script, it still works.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = plugins.run(id, dir, plugin_dirs, args).await {
        eprintln!("Error running plugin: {e}");
        std::process::exit(1);
    } else {
        std::process::exit(0);
    }
}

fn experimental_error_message(command: &str) -> anyhow::Result<CommandOutput> {
    bail!("The `wash {command}` command is experimental and may change in future releases. Set the `WASH_EXPERIMENTAL` environment variable or `--experimental` flag to `true` to use this command.")
}

/// Helper for loading plugins. If plugins fail to load, we log the error and return `None` so
/// execution can continue
async fn load_plugins() -> Option<(SubcommandRunner, PathBuf)> {
    // We need to use env vars here because the plugin loading needs to be initialized before
    // the CLI is parsed
    let plugin_dir = match WASH_DIRECTORIES.create_plugins_dir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::error!(err = ?e, "Could not load wash plugin directory");
            return None;
        }
    };

    let plugins = match wash::cli::util::load_plugins(&plugin_dir).await {
        Ok(plugins) => plugins,
        Err(e) => {
            tracing::error!(err = ?e, "Could not load wash plugins");
            return None;
        }
    };

    Some((plugins, plugin_dir))
}

async fn ensure_plugin_scratch_dir_exists(
    plugin_dir: impl AsRef<Path>,
    id: &str,
) -> anyhow::Result<PathBuf> {
    let dir = plugin_dir.as_ref().join("scratch").join(id);
    if !tokio::fs::try_exists(&dir).await.unwrap_or(false) {
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            bail!("Could not create plugin scratch directory: {e:?}");
        }
    }
    Ok(dir)
}

async fn inform_new_wash_version() -> anyhow::Result<()> {
    let wash_version = Version::parse(clap::crate_version!())
        .context("failed to parse version from current crate")?;
    let releases = get_wash_versions_newer_than(&wash_version)
        .await
        .context("failed to retrieve newer wash releases")?;
    if let Some(latest_release) = releases
        .first()
        .map(|x| x.get_x_y_z_version())
        .transpose()?
    {
        eprintln!(
            "{} Consider upgrading to newest wash version: v{latest_release}",
            emoji::INFO_SQUARE,
        );
    }
    Ok(())
}

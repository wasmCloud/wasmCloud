// Increase the default recursion limit
#![recursion_limit = "256"]

use std::{
    io::{BufWriter, IsTerminal},
    path::PathBuf,
};

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use clap_complete::generate;
use tracing::{Level, error, info, instrument, trace, warn};

use wash::cli::{
    CONFIG_DIR_NAME, CONFIG_FILE_NAME, CliCommand, CliContext, CommandOutput, OutputKind,
};

#[derive(Debug, Clone, Parser)]
#[command(
    name = "wash",
    about,
    version,
    arg_required_else_help = true,
    color = clap::ColorChoice::Auto
)]
struct Cli {
    /// Specify output format (text or json)
    #[arg(short = 'o', long = "output", default_value = "text", global = true)]
    pub(crate) output: OutputKind,

    /// Print help in markdown format (conflicts with --help and --output json)
    #[arg(long = "help-markdown", hide = true, global = true)]
    help_markdown: bool,

    /// Set the opentelemetry log level (trace, debug, info, warn, error)
    #[arg(short = 'l', long = "log-level", default_value_t = Level::INFO, global = true)]
    log_level: Level,

    /// Enable verbose output
    #[arg(long = "verbose", global = true)]
    verbose: bool,

    /// Run in non-interactive mode: skip terminal checks for host exec and accept the default answer for any confirmation prompts. Automatically enabled when stdin is not a TTY
    #[arg(long = "non-interactive", global = true)]
    non_interactive: bool,

    /// Path to user configuration file
    #[arg(long = "user-config", global = true)]
    user_config: Option<PathBuf>,

    /// Path to the project directory
    #[arg(short = 'C', default_value = find_project_root().into_os_string())]
    project_path: PathBuf,

    /// What to measure about guests: `off`, `duration`, or `fuel`.
    ///
    /// `duration` reports how many invocations there were, how many failed and
    /// how long they took, for two clock reads a call — the default, because it
    /// is what an operator needs first and costs the guest nothing. `fuel` adds
    /// an exact count of the operations a guest executed, at the price of a
    /// counter compiled into every block of its code.
    #[arg(long = "meters", global = true, default_value = "duration")]
    meters: wash_runtime::observability::MeterKind,

    /// Deprecated spelling of `--meters fuel`.
    #[arg(
        long = "enable-meters",
        global = true,
        hide = true,
        conflicts_with = "meters"
    )]
    enable_meters: bool,

    #[command(subcommand)]
    command: Option<WashCliCommand>,
}

impl Cli {
    /// The meter to run, honoring the deprecated `--enable-meters`, which turned
    /// on the fuel counter.
    fn meters(&self) -> wash_runtime::observability::MeterKind {
        if self.enable_meters {
            wash_runtime::observability::MeterKind::Fuel
        } else {
            self.meters
        }
    }
}

/// The main CLI commands for wash
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Subcommand)]
enum WashCliCommand {
    /// Build a Wasm component
    Build(wash::cli::component_build::ComponentBuildCommand),
    /// Generate shell completions
    Completion(wash::cli::completion::CompletionCommand),
    /// View configuration for wash
    Config(wash::cli::config::ConfigArgs),
    /// Start a development server for a Wasm component
    Dev(wash::cli::dev::DevCommand),
    /// Inspect a Wasm component's embedded WIT
    Inspect(wash::cli::inspect::InspectCommand),
    /// Act as a Host
    Host(wash::cli::host::HostCommand),
    /// Create a new project from a template or git repository
    New(wash::cli::new::NewCommand),
    /// Push or pull Wasm components to/from an OCI registry
    #[command(alias = "docker")]
    Oci(wash::cli::oci::OciArgs),
    /// Update wash to the latest version
    #[command(alias = "upgrade")]
    Update(wash::cli::update::UpdateCommand),
    /// Manage WIT dependencies
    Wit(wash::cli::wit::WitArgs),
}

impl CliCommand for WashCliCommand {
    /// Handle the wash command
    #[instrument(level = "debug", skip_all, name = "wash")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            WashCliCommand::Build(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Completion(cmd) => {
                // Handle completion generation directly here since we need access to the full CLI
                let mut wash_cmd = Cli::command();

                let cli_name = wash_cmd.get_name().to_owned();
                generate(cmd.shell(), &mut wash_cmd, cli_name, &mut std::io::stdout());

                Ok(CommandOutput::ok("", None))
            }
            WashCliCommand::Config(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Dev(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Inspect(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Host(cmd) => cmd.handle(ctx).await,
            WashCliCommand::New(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Oci(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Update(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Wit(cmd) => cmd.handle(ctx).await,
        }
    }
}

#[tokio::main]
async fn main() {
    let global_parser = Cli::command_for_update()
        .arg_required_else_help(false)
        .subcommand_required(false)
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .ignore_errors(true)
        .get_matches();
    // Use unwrap_or to provide defaults when the first parse fails.
    // This can happen when a nested subcommand is missing (e.g., `wash config`
    // without a subcommand like `init`). By falling through with defaults,
    // the second parse below can show proper help via arg_required_else_help.
    let global_args = Cli::from_arg_matches(&global_parser).unwrap_or(Cli {
        output: OutputKind::Text,
        help_markdown: false,
        log_level: Level::INFO,
        verbose: false,
        non_interactive: false,
        user_config: None,
        project_path: find_project_root(),
        meters: wash_runtime::observability::MeterKind::Off,
        enable_meters: false,
        command: None,
    });

    // Check for --non-interactive flag before parsing (to avoid requiring plugin commands to be registered)
    let non_interactive_flag = global_args.non_interactive;

    // Auto-detect non-interactive mode if stdin is not a TTY or flag is set
    let non_interactive = non_interactive_flag || !std::io::stdin().is_terminal();

    let (mut stdout, mut stderr) = (Box::new(std::io::stdout()), Box::new(std::io::stderr()));

    // For `wash dev`, export `dev.environment` from the project config into the
    // wash process before observability initializes. The tracing subscriber reads
    // `RUST_LOG` and the OTel exporter checks for `OTEL_*` once during
    // `initialize_observability`. Neither honors changes made later, so values
    // set inside `DevCommand::handle()` arrive too late.
    if matches!(global_args.command, Some(WashCliCommand::Dev(_))) {
        wash::config::apply_dev_environment(
            global_args.user_config.as_deref(),
            &global_args.project_path,
        );
    }

    // Initialize observability as early as possible, with the specified log level
    let observability_shutdown = wash_runtime::observability::initialize_observability(
        global_args.log_level,
        !non_interactive,
        global_args.verbose,
    )
    .unwrap_or_else(|e| {
        exit_with_output(
            &mut stderr,
            CommandOutput::error(format!("failed to initialize observability: {e:?}"), None)
                .with_output_kind(global_args.output),
        );
    });

    if global_args.enable_meters {
        warn!("`--enable-meters` is deprecated, use `--meters fuel` instead");
    }

    // Check if project path exists
    if !global_args.project_path.exists() {
        exit_with_output(
            &mut stderr,
            CommandOutput::error(
                format!("{:?} does not exist", global_args.project_path),
                None,
            )
            .with_output_kind(global_args.output),
        );
    }

    let project_absolute_path = match std::fs::canonicalize(&global_args.project_path) {
        Ok(path) => path,
        Err(e) => {
            exit_with_output(
                &mut stderr,
                CommandOutput::error(
                    format!(
                        "failed to canonicalize project path {:?}: {}",
                        global_args.project_path, e
                    ),
                    None,
                )
                .with_output_kind(global_args.output),
            );
        }
    };

    // ************* WARNING *************
    // From now on relative paths will be relative to the project path
    // ***********************************

    let wash_cmd = Cli::command();
    // Create global context with output kind and directory paths
    let mut ctx_builder = CliContext::builder()
        .non_interactive(non_interactive)
        .project_dir(project_absolute_path)
        .meters(global_args.meters());

    // Load custom config if provided, otherwise will default to XDG config path
    if let Some(config_path) = global_args.user_config {
        if !config_path.exists() {
            exit_with_output(
                &mut stderr,
                CommandOutput::error(format!("{config_path:?} does not exist"), None)
                    .with_output_kind(global_args.output),
            );
        }

        ctx_builder = ctx_builder.config(config_path)
    }

    let ctx = match ctx_builder.build().await {
        Ok(ctx) => ctx,
        Err(e) => {
            error!(error = ?e, "failed to infer global context");
            // In the rare case that this fails, we'll parse and initialize the CLI here to output properly.
            exit_with_output(
                &mut stdout,
                CommandOutput::error(format!("{e:?}"), None).with_output_kind(global_args.output),
            );
        }
    };
    trace!(ctx = ?ctx, "inferred global context");

    let help_cmd = wash_cmd.clone();
    let matches = wash_cmd.get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    trace!(cli = ?cli, "parsed CLI");

    // Implements clap_markdown for markdown generation of command line documentation. Most straightforward way to invoke is probably `wash app get --help-markdown > help.md`
    if cli.help_markdown {
        let help_output = clap_markdown::help_markdown_command(&help_cmd);
        println!("{help_output}");
        wash_runtime::observability::flush();
        std::process::exit(0);
    }

    // Use a buffered writer to prevent broken pipe errors
    let mut stdout_buf = BufWriter::new(stdout);

    // Recommend a new version of wash if available
    // Don't show the update message if the user is updating
    if !non_interactive && !matches!(cli.command, Some(WashCliCommand::Update(_))) {
        match ctx.check_new_version().await {
            Ok(version) => info!(
                new_version = %version,
                "a new version of wash is available! Update to the latest version with `wash update`"
            ),
            Err(e) => trace!(error = ?e, "version check"),
        }
    }

    let command_output = if let Some(command) = cli.command {
        run_command(ctx, command).await
    } else {
        Ok(CommandOutput::error(
            "No command provided. Use `wash --help` to see available commands.",
            None,
        ))
    };

    observability_shutdown();

    exit_with_output(
        &mut stdout_buf,
        command_output
            .unwrap_or_else(|e| {
                // NOTE: This format!() invocation specifically outputs the anyhow backtrace, which is why
                // it's used over a `.to_string()` call.
                CommandOutput::error(format!("{e:?}"), None).with_output_kind(global_args.output)
            })
            .with_output_kind(global_args.output),
    )
}

/// Helper function to execute a command that impl's [`CliCommand`], returning the output
async fn run_command<C>(ctx: CliContext, command: C) -> anyhow::Result<CommandOutput>
where
    C: CliCommand + std::fmt::Debug,
{
    trace!(command = ?command, "handling command");
    command.handle(&ctx).await
}

/// Helper function to ensure that we're exiting the program consistently and with the correct output format.
#[allow(clippy::expect_used)] // Panicking on stdout failure during exit is acceptable
fn exit_with_output(stdout: &mut impl std::io::Write, output: CommandOutput) -> ! {
    // Every early exit above reaches the process's end through here, after
    // `initialize_observability`; without this they drop what the exporters
    // still hold. Idempotent, so the success path's own call stands.
    wash_runtime::observability::flush();
    let (message, success) = output.render();
    writeln!(stdout, "{message}").expect("failed to write output to stdout");
    stdout.flush().expect("failed to flush stdout");
    if success {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}

fn find_project_root() -> PathBuf {
    let fallback = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut current_dir = match fallback.canonicalize() {
        Ok(dir) => dir,
        Err(_) => return fallback,
    };

    loop {
        // Look for .wash/config.yaml (project config), not just .wash/ directory
        let project_config = current_dir.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME);
        if project_config.exists() {
            return current_dir;
        }

        if let Some(parent) = current_dir.parent() {
            current_dir = parent.to_path_buf();
        } else {
            break;
        }
    }

    fallback
}

#[cfg(test)]
mod tests {
    use super::*;
    use wash_runtime::observability::MeterKind;

    #[test]
    fn enable_meters_selects_fuel() {
        let cli = Cli::try_parse_from(["wash", "--enable-meters", "inspect", "component.wasm"])
            .expect("--enable-meters should parse");
        assert_eq!(cli.meters(), MeterKind::Fuel);
    }

    #[test]
    fn meters_flag_stands_alone() {
        let cli = Cli::try_parse_from(["wash", "--meters", "off", "inspect", "component.wasm"])
            .expect("--meters should parse");
        assert_eq!(cli.meters(), MeterKind::Off);

        // Rate, errors and duration without being asked: they cost the guest
        // two clock reads, and a host nobody can see is worse than one that is
        // marginally slower.
        let cli = Cli::try_parse_from(["wash", "inspect", "component.wasm"])
            .expect("no meter flag should parse");
        assert_eq!(cli.meters(), MeterKind::Duration);
    }

    #[test]
    fn both_meter_flags_conflict() {
        Cli::try_parse_from([
            "wash",
            "--enable-meters",
            "--meters",
            "epoch",
            "inspect",
            "component.wasm",
        ])
        .expect_err("the deprecated flag and its replacement should conflict");
    }
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::lib::app::{load_app_manifest, validate_manifest_file, AppManifest};
use crate::lib::cli::get::parse_watch_interval;
use crate::lib::cli::{CliConnectionOpts, CommandOutput, OutputKind};
use crate::lib::config::WashConnectionOptions;
use anyhow::{bail, Context};
use async_nats::RequestErrorKind;
use clap::{Args, Subcommand};
use serde_json::json;
use wadm_client::Result;
use wadm_types::api::ModelSummary;
use wadm_types::validation::{ValidationFailure, ValidationOutput};

use crate::appearance::spinner::Spinner;
use crossterm::{
    cursor, execute,
    terminal::{Clear, ClearType},
};
use std::io::Write;

mod output;

#[derive(Debug, Clone, Subcommand)]
pub enum AppCliCommand {
    /// List all applications available within the lattice
    #[clap(name = "list")]
    List(ListCommand),
    /// Get the application manifest for a specific version of an application
    #[clap(name = "get")]
    Get(GetCommand),
    /// Get the current status of a given application
    #[clap(name = "status")]
    Status(StatusCommand),
    /// Get the version history of a given application
    #[clap(name = "history")]
    History(HistoryCommand),
    /// Delete an application version
    #[clap(name = "delete", alias = "del")]
    Delete(DeleteCommand),
    /// Create an application version by putting the manifest into the wadm store
    #[clap(name = "put")]
    Put(PutCommand),
    /// Deploy an application to the lattice
    #[clap(name = "deploy")]
    Deploy(DeployCommand),
    /// Undeploy an application, removing it from the lattice
    #[clap(name = "undeploy")]
    Undeploy(UndeployCommand),
    /// Validate an application manifest
    #[clap(name = "validate")]
    Validate(ValidateCommand),
}

#[derive(Args, Debug, Clone)]
pub struct ListCommand {
    #[clap(flatten)]
    opts: CliConnectionOpts,

    /// Enables Real-time updates, duration can be specified in ms or in humantime (eg: 5s, 2m, 15ms). Defaults to 1000 milliseconds.
    #[clap(long,short, num_args = 0..=1, default_missing_value = "1000", value_parser = parse_watch_interval)]
    pub watch: Option<std::time::Duration>,
}

#[derive(Args, Debug, Clone)]
pub struct UndeployCommand {
    /// Name of the application to undeploy
    #[clap(name = "name", required_unless_present("all"))]
    app_name: Option<String>,

    #[clap(flatten)]
    opts: CliConnectionOpts,

    /// Whether to undeploy all the available apps
    #[clap(long = "all", default_value = "false")]
    all: bool,
}

#[derive(Args, Debug, Clone)]
pub struct DeployCommand {
    /// Name of the application to deploy, if it was already `put`, or a path to a file containing the application manifest
    #[clap(name = "application")]
    app_name: Option<String>,

    /// Version of the application to deploy, defaults to the latest created version
    #[clap(name = "version")]
    version: Option<String>,

    /// Whether or not wash should attempt to replace the resources by performing an optimistic delete shortly before applying resources.
    #[clap(long = "replace")]
    replace: bool,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub struct DeleteCommand {
    /// Name of the application to delete, or a path to a Wadm Application Manifest
    #[clap(name = "name", required_unless_present("all_undeployed"))]
    app_name: Option<String>,

    /// Version of the application to delete. If not supplied, all versions are deleted
    #[clap(name = "version")]
    version: Option<String>,

    #[clap(flatten)]
    opts: CliConnectionOpts,

    /// Whether to delete all undeployed apps
    #[clap(long = "all-undeployed", default_value = "false")]
    all_undeployed: bool,
}

#[derive(Args, Debug, Clone)]
pub struct PutCommand {
    /// The source of the application manifest, either a file path, remote file http url, or stdin. If no source is provided (or arg marches '-'), stdin is used.
    source: Option<String>,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

/// Command to get the application manifest(s)
#[derive(Args, Debug, Clone)]
pub struct GetCommand {
    /// The name of the application to retrieve.
    ///
    /// If left empty retrieves all the applications, same as `wash app list`
    #[clap(name = "name")]
    app_name: Option<String>,

    /// The version of the application to retrieve. If left empty, retrieves the latest version
    #[clap(name = "version")]
    version: Option<String>,

    /// Enables real-time updates.
    ///
    /// Duration can be specified in ms (as number) or in [humantime](https://docs.rs/humantime) (eg: 5s, 2m, 15ms). Defaults to 1s.
    #[clap(long,short, num_args = 0..=1, default_missing_value = "1s", value_parser = parse_watch_interval)]
    pub watch: Option<std::time::Duration>,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub struct StatusCommand {
    /// The name of the application
    #[clap(name = "name")]
    app_name: String,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub struct HistoryCommand {
    /// The name of the application
    #[clap(name = "name")]
    app_name: String,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub struct ValidateCommand {
    /// Path to the application manifest to validate
    #[clap(name = "application")]
    application: PathBuf,
    /// Whether to check image references in the manifest
    #[clap(long)]
    check_image_refs: bool,
}

pub async fn handle_command(
    command: AppCliCommand,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    use AppCliCommand::{Delete, Deploy, Get, History, List, Put, Status, Undeploy, Validate};
    let sp: Spinner = Spinner::new(&output_kind)?;
    let command_output: wadm_client::Result<CommandOutput> = match command {
        List(cmd) => {
            sp.update_spinner_message("Listing applications ...".to_string());
            get_application_list(cmd, &sp).await
        }
        Get(cmd) => {
            if let Some(app_name) = cmd.clone().app_name {
                sp.update_spinner_message("Getting application... ".to_string());
                get_manifest(cmd, &app_name).await
            } else {
                sp.update_spinner_message("Getting application manifests... ".to_string());
                get_applications(cmd, &sp).await
            }
        }
        Status(cmd) => {
            sp.update_spinner_message("Getting application status ... ".to_string());
            get_model_status(cmd).await
        }
        History(cmd) => {
            sp.update_spinner_message("Getting application version history ... ".to_string());
            get_application_versions(cmd).await
        }
        Delete(cmd) => {
            sp.update_spinner_message("Deleting application version ... ".to_string());
            delete_application_version(cmd).await
        }
        Put(cmd) => {
            sp.update_spinner_message("Creating application version ... ".to_string());
            put_model(cmd).await
        }
        Deploy(cmd) => {
            sp.update_spinner_message("Deploying application ... ".to_string());
            deploy_model(cmd).await
        }
        Undeploy(cmd) => {
            sp.update_spinner_message("Undeploying application ... ".to_string());
            undeploy_model(cmd).await
        }
        Validate(cmd) => {
            sp.update_spinner_message("Validating application manifest ... ".to_string());
            handle_validate(cmd).await
        }
    };

    // Basic match to give a nicer error than "no responders"
    match command_output {
        Err(wadm_client::error::ClientError::NatsError(e))
            if e.kind() == RequestErrorKind::NoResponders =>
        {
            bail!("Connection succeeded to lattice but no wadm server was listening. Ensure wadm is running.")
        }
        _ => {}
    }

    sp.finish_and_clear();

    Ok(command_output?)
}
/// Validate a Wadm manifest file
async fn handle_validate(cmd: ValidateCommand) -> Result<CommandOutput> {
    let (_manifest, validation_results) =
        validate_manifest_file(&cmd.application, cmd.check_image_refs)
            .await
            .context("failed to validate Wadm manifest")?;
    Ok(show_validate_manifest_results(validation_results))
}

async fn undeploy_model(cmd: UndeployCommand) -> Result<CommandOutput> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());
    let client = connection_opts.into_nats_client().await?;

    // Determine which models to remove, if a single model is not specified,
    // then attempt to filter the list of existing models
    let models = match cmd.app_name {
        // If an explicit app name was specified, resolve the right app name and version
        Some(app_name) => {
            // If we have received a valid path to a model file, then read and extract the model name,
            // otherwise use the supplied name as a model name
            let model_name = if tokio::fs::try_exists(&app_name)
                .await
                .is_ok_and(|exists| exists)
            {
                let manifest = load_app_manifest(app_name.parse()?)
                    .await
                    .with_context(|| format!("failed to load app manifest at [{app_name}]"))?;
                manifest
                    .name()
                    .map(ToString::to_string)
                    .context("failed to find name of manifest")?
            } else {
                app_name
            };

            vec![model_name]
        }
        // If no model name was specified, use command-specified filters to determine which models to act on
        None if cmd.all => crate::lib::app::get_models(&client, lattice.clone())
            .await?
            .into_iter()
            .map(|m| m.name)
            .collect(),
        _ => Vec::new(),
    };

    let mut undeployed = Vec::new();
    let mut output_map = HashMap::new();

    // Undeploy models
    for model_name in &models {
        match crate::lib::app::undeploy_model(&client, lattice.clone(), model_name).await {
            Ok(()) => undeployed.push(model_name),
            Err(e) => eprintln!("failed to undeploy model [{model_name}]: {e}"),
        }
    }

    let output_msg = match &models[..] {
        [] => "No applications undeployed".into(),
        [m] => format!("Undeployed application: {m}"),
        _ => format!("Undeployed [{}] applications", undeployed.len()),
    };
    output_map.insert("results".to_string(), json!(output_msg));
    output_map.insert(
        "undeployed_application_names".to_string(),
        json!(undeployed),
    );
    Ok(CommandOutput::new(output_msg, output_map))
}

async fn deploy_model(cmd: DeployCommand) -> Result<CommandOutput> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    let app_manifest = match cmd.app_name {
        Some(source) if source == "-" => load_app_manifest("-".parse()?).await?,
        Some(source) => load_app_manifest(source.parse()?).await?,
        None => {
            return Err(wadm_client::error::ClientError::ManifestLoad(
                anyhow::anyhow!(
                    "Missing manifest name/path. To load a manifest from STDIN, please pass '-'"
                ),
            ))
        }
    };

    // If --replace was specified, we should attempt to replace the resources by deleting them beforehand
    if cmd.replace {
        if let (Some(name), version) = (
            app_manifest.name(),
            app_manifest.version().map(ToString::to_string),
        ) {
            if let Err(e) =
                crate::lib::app::delete_model_version(&client, lattice.clone(), name, version).await
            {
                eprintln!("🟨 Failed to delete model during replace operation: {e}");
            }
        }
    }

    deploy_model_from_manifest(&client, lattice, app_manifest, cmd.version).await
}

pub(crate) async fn deploy_model_from_manifest(
    client: &async_nats::Client,
    lattice: Option<String>,
    manifest: AppManifest,
    version: Option<String>,
) -> Result<CommandOutput> {
    let (name, version) = match manifest {
        AppManifest::SerializedModel(manifest) => crate::lib::app::put_and_deploy_model(
            client,
            lattice,
            serde_yaml::to_string(&manifest)
                .context("failed to convert manifest to string")?
                .as_ref(),
        )
        .await
        .map(|(name, version)| (name, Some(version))),
        AppManifest::ModelName(model_name) => {
            crate::lib::app::deploy_model(client, lattice, &model_name, version.clone()).await
        }
    }?;

    let mut map = HashMap::new();
    let version = version.unwrap_or_default();
    map.insert("deployed".to_string(), json!(true));
    map.insert("model_name".to_string(), json!(name));
    map.insert("model_version".to_string(), json!(version));
    Ok(CommandOutput::new(
        format!("Deployed application \"{name}\", version \"{version}\""),
        map,
    ))
}

async fn put_model(cmd: PutCommand) -> Result<CommandOutput> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    let app_manifest = match &cmd.source {
        Some(source) => load_app_manifest(source.parse()?).await?,
        None => load_app_manifest("-".parse()?).await?,
    };

    let (name, version) = match app_manifest {
        AppManifest::SerializedModel(manifest) => crate::lib::app::put_model(
            &client,
            lattice,
            serde_yaml::to_string(&manifest)
                .context("failed to convert manifest to string")?
                .as_ref(),
        )
        .await
        .map_err(|e| anyhow::anyhow!(e)),
        AppManifest::ModelName(name) => {
            return Err(wadm_client::error::ClientError::ManifestLoad(anyhow::anyhow!("failed to retrieve manifest. Ensure `{name}` is a valid path to a Wadm application manifest.")));
        }
    }?;

    let mut map = HashMap::new();
    map.insert("deployed".to_string(), json!(true));
    map.insert("model_name".to_string(), json!(name));
    map.insert("model_version".to_string(), json!(version));
    Ok(CommandOutput::new(
        format!("Put application \"{name}\", version \"{version}\""),
        map,
    ))
}

async fn get_application_versions(cmd: HistoryCommand) -> Result<CommandOutput> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    let versions = crate::lib::app::get_model_history(&client, lattice, &cmd.app_name).await?;
    let mut map = HashMap::new();
    map.insert("revisions".to_string(), json!(versions));
    Ok(CommandOutput::new(
        output::list_revisions_table(versions),
        map,
    ))
}

async fn get_model_status(cmd: StatusCommand) -> Result<CommandOutput> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    let status = crate::lib::app::get_model_status(&client, lattice, &cmd.app_name).await?;

    let mut map = HashMap::new();
    map.insert("status".to_string(), json!(status));
    Ok(CommandOutput::new(
        output::status_table(cmd.app_name, status),
        map,
    ))
}

async fn get_manifest(cmd: GetCommand, app_name: &str) -> Result<CommandOutput> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    let manifest =
        crate::lib::app::get_model_details(&client, lattice, app_name, cmd.version).await?;

    let mut map = HashMap::new();
    map.insert("application".to_string(), json!(manifest));
    let yaml = serde_yaml::to_string(&manifest).unwrap();
    Ok(CommandOutput::new(yaml, map))
}

async fn delete_application_version(cmd: DeleteCommand) -> Result<CommandOutput> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    // Determine which models to remove, if a single model is not specified,
    // then attempt to filter the list of existing models
    let models = match cmd.app_name {
        // If an explicit app name was specified, resolve the right app name and version
        Some(app_name) => {
            // If we have received a valid path to a model file, then read and extract the model name,
            // otherwise use the supplied name as a model name
            let (model_name, version): (String, Option<String>) =
                if tokio::fs::try_exists(&app_name)
                    .await
                    .is_ok_and(|exists| exists)
                {
                    let manifest = load_app_manifest(app_name.parse()?)
                        .await
                        .with_context(|| format!("failed to load app manifest at [{app_name}]"))?;
                    (
                        manifest
                            .name()
                            .map(ToString::to_string)
                            .context("failed to find name of manifest")?,
                        manifest.version().map(ToString::to_string),
                    )
                } else {
                    (app_name, cmd.version)
                };

            vec![(model_name, version)]
        }
        // If no model name was specified, use command-specified filters to determine which models to act on
        None if cmd.all_undeployed => crate::lib::app::get_models(&client, lattice.clone())
            .await?
            .into_iter()
            .filter_map(|m| match m.detailed_status.info.status_type {
                wadm_types::api::StatusType::Undeployed => Some((m.name, Some(m.version))),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };

    let mut deleted_models = Vec::new();

    #[derive(serde::Serialize)]
    struct ModelNameAndVersion<'a> {
        model_name: &'a String,
        version: &'a Option<String>,
    }

    // Delete all specified models
    for (model_name, version) in &models {
        match crate::lib::app::delete_model_version(
            &client,
            lattice.clone(),
            model_name,
            version.clone(),
        )
        .await
        {
            Ok(true) => deleted_models.push(ModelNameAndVersion {
                model_name,
                version,
            }),
            // Deletion failure normally implies that the model has already been deleted
            Ok(false) => {}
            Err(e) => {
                eprintln!("failed to delete model [{model_name}]: {e}");
            }
        }
    }

    let mut output_map = HashMap::new();
    let output_msg = match models[..] {
        [] => "No applications deleted".into(),
        [(ref model_name, _)] => {
            output_map.insert("deleted".to_string(), json!(true));
            if deleted_models.len() == 1 {
                format!("Deleted application: {model_name}")
            } else {
                format!("Already deleted application: {model_name}")
            }
        }
        _ => {
            output_map.insert("deleted_applications".into(), json!(deleted_models));
            format!("Deleted [{}] applications", deleted_models.len())
        }
    };

    Ok(CommandOutput::new(output_msg, output_map))
}

async fn get_application_list(cmd: ListCommand, sp: &Spinner) -> Result<CommandOutput> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    if cmd.watch.is_some() {
        sp.finish_and_clear();
        watch_applications(&client, lattice, cmd.watch).await?;
        Ok(CommandOutput::new(
            "Completed Watching Applications".to_string(),
            HashMap::new(),
        ))
    } else {
        let models = crate::lib::app::get_models(&client, lattice).await?;
        let mut map = HashMap::new();
        map.insert("applications".to_string(), json!(models));
        Ok(CommandOutput::new(output::list_models_table(models), map))
    }
}

async fn get_applications(cmd: GetCommand, sp: &Spinner) -> Result<CommandOutput> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    if cmd.watch.is_some() {
        sp.finish_and_clear();
        watch_applications(&client, lattice, cmd.watch).await?;
        Ok(CommandOutput::new(
            "Completed Watching Applications".to_string(),
            HashMap::new(),
        ))
    } else {
        let models = crate::lib::app::get_models(&client, lattice).await?;
        let mut map = HashMap::new();
        map.insert("applications".to_string(), json!(models));
        Ok(CommandOutput::new(output::list_models_table(models), map))
    }
}

async fn watch_applications(
    client: &async_nats::Client,
    lattice: Option<String>,
    watch: Option<Duration>,
) -> Result<()> {
    let mut stdout = std::io::stdout();

    execute!(stdout, Clear(ClearType::FromCursorUp), cursor::MoveTo(0, 0))
        .map_err(|e| anyhow::anyhow!("Failed to clear terminal: {}", e))?;

    let mut ctrlc = std::pin::pin!(tokio::signal::ctrl_c());
    let watch_interval = watch.unwrap_or(Duration::from_millis(1000));

    loop {
        let models = tokio::select! {
            res = crate::lib::app::get_models(client, lattice.clone()) => res?,
            _res = &mut ctrlc => {
                execute!(stdout, Clear(ClearType::Purge), Clear(ClearType::FromCursorUp), cursor::MoveTo(0, 0), cursor::Show)
                    .map_err(|e| anyhow::anyhow!("Failed to execute terminal commands: {}", e))?;
                stdout.flush()
                    .map_err(|e| anyhow::anyhow!("Failed to flush stdout: {}", e))?;
                return Ok(());
            }
        };

        let table = output::list_models_table(models);

        execute!(stdout, Clear(ClearType::Purge), cursor::MoveTo(0, 0))
            .map_err(|e| anyhow::anyhow!("Failed to execute terminal commands: {}", e))?;

        stdout
            .write_all(table.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to write table to stdout: {}", e))?;

        stdout
            .flush()
            .map_err(|e| anyhow::anyhow!("Failed to flush stdout: {}", e))?;

        execute!(
            stdout,
            Clear(ClearType::CurrentLine),
            Clear(ClearType::FromCursorDown),
        )
        .map_err(|e| anyhow::anyhow!("Failed to clear terminal: {}", e))?;

        tokio::select! {
            () = tokio::time::sleep(watch_interval) => continue,
            _res = &mut ctrlc => {
                execute!(stdout, Clear(ClearType::Purge), Clear(ClearType::FromCursorUp), cursor::MoveTo(0, 0), cursor::Show)
                    .map_err(|e| anyhow::anyhow!("Failed to execute terminal commands: {}", e))?;
                stdout.flush()
                    .map_err(|e| anyhow::anyhow!("Failed to flush stdout: {}", e))?;
                return Ok(());
            }
        }
    }
}

fn show_validate_manifest_results(messages: impl AsRef<[ValidationFailure]>) -> CommandOutput {
    let messages = messages.as_ref();
    let valid = messages.valid();
    let warnings = messages
        .warnings()
        .into_iter()
        .cloned()
        .collect::<Vec<ValidationFailure>>();
    let errors = messages
        .errors()
        .into_iter()
        .cloned()
        .collect::<Vec<ValidationFailure>>();
    let message = if valid {
        "manifest is valid".into()
    } else {
        format!(
            r"invalid manifest:
warnings: {warnings:#?}
errors: {errors:#?}
"
        )
    };
    let json_output = HashMap::<String, serde_json::Value>::from([
        ("valid".into(), messages.valid().into()),
        ("warnings".into(), json!(warnings)),
        ("errors".into(), json!(errors)),
    ]);
    CommandOutput::new(message, json_output)
}

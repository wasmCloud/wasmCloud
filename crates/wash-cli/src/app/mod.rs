use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde_json::json;
use wadm::server::{
    DeleteModelResponse, DeployModelResponse, GetModelResponse, GetResult, ModelSummary,
    PutModelResponse, PutResult, StatusResponse, VersionResponse,
};
use wash_lib::app::{load_app_manifest, AppManifest};
use wash_lib::cli::{CliConnectionOpts, CommandOutput, OutputKind};
use wash_lib::config::WashConnectionOptions;

use crate::appearance::spinner::Spinner;

mod output;

#[derive(Debug, Clone, Subcommand)]
pub enum AppCliCommand {
    /// List application specifications available within the lattice
    #[clap(name = "list")]
    List(ListCommand),
    /// Retrieve the details for a specific version of an app specification
    #[clap(name = "get")]
    Get(GetCommand),
    /// Retrieve the status of a given model within the lattice
    #[clap(name = "status")]
    Status(StatusCommand),
    /// Retrieve the version history of a given model within the lattice
    #[clap(name = "history")]
    History(HistoryCommand),
    /// Delete a model version
    #[clap(name = "delete", alias = "del")]
    Delete(DeleteCommand),
    /// Puts a model version into the store
    #[clap(name = "put")]
    Put(PutCommand),
    /// Deploy an application to the lattice
    #[clap(name = "deploy")]
    Deploy(DeployCommand),
    /// Undeploy an application, removing it from the lattice
    #[clap(name = "undeploy")]
    Undeploy(UndeployCommand),
}

#[derive(Args, Debug, Clone)]
pub struct ListCommand {
    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub struct UndeployCommand {
    /// Name of the app specification to undeploy
    #[clap(name = "name")]
    model_name: String,

    /// Whether or not to delete resources that are undeployed. Defaults to remove managed resources
    #[clap(long = "non-destructive")]
    non_destructive: bool,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub struct DeployCommand {
    /// Name of the application to deploy, if it was already `put`, or a path to a file containing the application manifest
    #[clap(name = "application")]
    application: Option<String>,

    /// Version of the app specification to deploy, defaults to the latest created version
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
    /// Name of the app specification to delete, or a path to a WADM Application Manifest
    #[clap(name = "name")]
    model_name: String,

    #[clap(long = "delete-all")]
    /// Whether or not to delete all app versions, defaults to `false`
    delete_all: bool,

    /// Version of the app specification to delete. Not required if --delete-all is supplied
    #[clap(name = "version")]
    version: Option<String>,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub struct PutCommand {
    /// Possible sources: file from fs, remote file http url,  or stdin. if no source is provided (or arg marches '-'), stdin is used.
    source: Option<String>,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub struct GetCommand {
    /// The name of the app spec to retrieve
    #[clap(name = "name")]
    model_name: String,

    /// The version of the app spec to retrieve. If left empty, retrieves the latest version
    #[clap(name = "version")]
    version: Option<String>,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub struct StatusCommand {
    /// The name of the app spec
    #[clap(name = "name")]
    model_name: String,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub struct HistoryCommand {
    /// The name of the app spec
    #[clap(name = "name")]
    model_name: String,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

pub async fn handle_command(
    command: AppCliCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    use AppCliCommand::*;
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = match command {
        List(cmd) => {
            sp.update_spinner_message("Querying app spec list ...".to_string());
            let results = get_models(cmd).await?;
            list_models_output(results)
        }
        Get(cmd) => {
            sp.update_spinner_message("Querying app spec details ... ".to_string());
            let results = get_model_details(cmd).await?;
            show_model_output(results)
        }
        Status(cmd) => {
            sp.update_spinner_message("Querying app status ... ".to_string());
            let model_name = cmd.model_name.clone();
            let results = get_model_status(cmd).await?;
            show_model_status(model_name, results)
        }
        History(cmd) => {
            sp.update_spinner_message("Querying app revision history ... ".to_string());
            let results = get_model_history(cmd).await?;
            show_model_history(results)
        }
        Delete(cmd) => {
            sp.update_spinner_message("Deleting app version ... ".to_string());
            let results = delete_model_version(cmd).await?;
            show_del_results(results)
        }
        Put(cmd) => {
            sp.update_spinner_message("Uploading app specification ... ".to_string());
            let results = put_model(cmd).await?;
            show_put_results(results)
        }
        Deploy(cmd) => {
            sp.update_spinner_message("Deploying application ... ".to_string());
            let results = deploy_model(cmd).await?;
            show_deploy_results(results)
        }
        Undeploy(cmd) => {
            sp.update_spinner_message("Undeploying application ... ".to_string());
            let results = undeploy_model(cmd).await?;
            show_undeploy_results(results)
        }
    };
    sp.finish_and_clear();

    Ok(out)
}

async fn undeploy_model(cmd: UndeployCommand) -> Result<DeployModelResponse> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    wash_lib::app::undeploy_model(&client, lattice, &cmd.model_name, cmd.non_destructive).await
}

async fn deploy_model(cmd: DeployCommand) -> Result<DeployModelResponse> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    let app_manifest = match cmd.application {
        Some(source) => load_app_manifest(source.parse()?).await?,
        None => load_app_manifest("-".parse()?).await?,
    };

    // If --replace was specified, we should attempt to replace the resources by deleting them beforehand
    if cmd.replace {
        if let (Some(name), version) = (app_manifest.name(), app_manifest.version().map(Into::into))
        {
            if let Err(e) =
                wash_lib::app::delete_model_version(&client, lattice.clone(), name, version, false)
                    .await
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
) -> Result<DeployModelResponse> {
    match manifest {
        AppManifest::SerializedModel(manifest) => {
            let put_res = wash_lib::app::put_model(
                client,
                lattice.clone(),
                serde_yaml::to_string(&manifest)
                    .context("failed to convert manifest to string")?
                    .as_ref(),
            )
            .await?;

            let model_name = match put_res.result {
                PutResult::Created | PutResult::NewVersion => put_res.name,
                _ => bail!("Could not put manifest to deploy {}", put_res.message),
            };
            wash_lib::app::deploy_model(client, lattice, &model_name, version).await
        }
        AppManifest::ModelName(model_name) => {
            wash_lib::app::deploy_model(client, lattice, &model_name, version).await
        }
    }
}

async fn put_model(cmd: PutCommand) -> Result<PutModelResponse> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    let app_manifest = match &cmd.source {
        Some(source) => load_app_manifest(source.parse()?).await?,
        None => load_app_manifest("-".parse()?).await?,
    };

    match app_manifest {
        AppManifest::SerializedModel(manifest) => {
            wash_lib::app::put_model(
                &client,
                lattice,
                serde_yaml::to_string(&manifest)
                    .context("failed to convert manifest to string")?
                    .as_ref(),
            )
            .await
        }
        AppManifest::ModelName(_) => {
            bail!("failed to retrieve manifest at `{:?}`", cmd.source)
        }
    }
}

async fn get_model_history(cmd: HistoryCommand) -> Result<VersionResponse> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    wash_lib::app::get_model_history(&client, lattice, &cmd.model_name).await
}

async fn get_model_status(cmd: StatusCommand) -> Result<StatusResponse> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    wash_lib::app::get_model_status(&client, lattice, &cmd.model_name).await
}

async fn get_model_details(cmd: GetCommand) -> Result<GetModelResponse> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    wash_lib::app::get_model_details(&client, lattice, &cmd.model_name, cmd.version).await
}

async fn delete_model_version(cmd: DeleteCommand) -> Result<DeleteModelResponse> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;

    // If we have received a valid path to a model file, then read and extract the model name,
    // otherwise use the supplied name as a model name
    let (model_name, version): (String, Option<String>) = if tokio::fs::try_exists(&cmd.model_name)
        .await
        .is_ok_and(|exists| exists)
    {
        let manifest = load_app_manifest(cmd.model_name.parse()?)
            .await
            .with_context(|| format!("failed to load app manifest at [{}]", cmd.model_name))?;
        (
            manifest
                .name()
                .map(Into::into)
                .context("failed to find name of manifest")?,
            manifest.version().map(Into::into),
        )
    } else {
        (cmd.model_name, cmd.version)
    };

    // If we're deleting a model from either file or by name, and we don't know it's version
    // --delete-all must be set
    if version.is_none() && !cmd.delete_all {
        bail!("--delete-all must be specified when deleting models by name, without a version")
    }

    wash_lib::app::delete_model_version(&client, lattice, &model_name, version, cmd.delete_all)
        .await
}

async fn get_models(cmd: ListCommand) -> Result<Vec<ModelSummary>> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?;
    let lattice = Some(connection_opts.get_lattice());

    let client = connection_opts.into_nats_client().await?;
    wash_lib::app::get_models(&client, lattice).await
}

fn list_models_output(results: Vec<ModelSummary>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("apps".to_string(), json!(results));
    CommandOutput::new(output::list_models_table(results), map)
}

fn show_model_output(md: GetModelResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("model".to_string(), json!(md));
    if md.result == GetResult::Success {
        let yaml = serde_yaml::to_string(&md.manifest).unwrap();
        CommandOutput::new(yaml, map)
    } else {
        CommandOutput::new(md.message, map)
    }
}

fn show_put_results(results: PutModelResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("results".to_string(), json!(results));
    CommandOutput::new(results.message, map)
}

fn show_undeploy_results(results: DeployModelResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("results".to_string(), json!(results));
    CommandOutput::new(results.message, map)
}

fn show_del_results(results: DeleteModelResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("deleted".to_string(), json!(results));
    CommandOutput::new(results.message, map)
}

fn show_deploy_results(results: DeployModelResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("acknowledged".to_string(), json!(results));
    CommandOutput::new(results.message, map)
}

fn show_model_history(results: VersionResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("revisions".to_string(), json!(results));
    CommandOutput::new(output::list_revisions_table(results.versions), map)
}

fn show_model_status(model_name: String, results: StatusResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("status".to_string(), json!(results));
    CommandOutput::new(
        output::status_table(model_name, results.status.unwrap_or_default()),
        map,
    )
}

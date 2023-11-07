use std::collections::HashMap;

use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use serde_json::json;
use wadm::server::{
    DeleteModelResponse, DeployModelResponse, GetModelResponse, GetResult, ModelSummary,
    PutModelResponse, PutResult, VersionResponse,
};
use wash_lib::{
    app::{load_app_manifest, AppManifest},
    cli::{CliConnectionOpts, CommandOutput, OutputKind},
    config::WashConnectionOptions,
};

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

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub struct DeleteCommand {
    /// Name of the app specification to delete
    #[clap(name = "name")]
    model_name: String,

    #[clap(long = "delete-all")]
    /// Whether or not to delete all app versions, defaults to `false`
    delete_all: bool,

    /// Version of the app specification to delete. Not required if --delete-all is supplied
    #[clap(name = "version", required_unless_present("delete_all"))]
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
    let (lattice_prefix, client) =
        get_lattice_prefix_and_nats_client_from_cmd_opts(cmd.opts).await?;

    wash_lib::app::undeploy_model(
        &client,
        lattice_prefix,
        &cmd.model_name,
        cmd.non_destructive,
    )
    .await
}

async fn deploy_model(cmd: DeployCommand) -> Result<DeployModelResponse> {
    let (lattice_prefix, client) =
        get_lattice_prefix_and_nats_client_from_cmd_opts(cmd.opts).await?;

    let app_manifest = match cmd.application {
        Some(source) => load_app_manifest(source.parse()?).await?,
        None => load_app_manifest("-".parse()?).await?,
    };

    match app_manifest {
        AppManifest::SerializedModel(manifest) => {
            let put_res =
                wash_lib::app::put_model(&client, lattice_prefix.clone(), &manifest).await?;

            let model_name = match put_res.result {
                PutResult::Created | PutResult::NewVersion => put_res.name,
                _ => bail!("Could not put manifest to deploy {}", put_res.message),
            };
            wash_lib::app::deploy_model(&client, lattice_prefix, &model_name, cmd.version).await
        }
        AppManifest::ModelName(model_name) => {
            wash_lib::app::deploy_model(&client, lattice_prefix, &model_name, cmd.version).await
        }
    }
}

async fn put_model(cmd: PutCommand) -> Result<PutModelResponse> {
    let (lattice_prefix, client) =
        get_lattice_prefix_and_nats_client_from_cmd_opts(cmd.opts).await?;

    let app_manifest = match &cmd.source {
        Some(source) => load_app_manifest(source.parse()?).await?,
        None => load_app_manifest("-".parse()?).await?,
    };

    match app_manifest {
        AppManifest::SerializedModel(manifest) => {
            wash_lib::app::put_model(&client, lattice_prefix, &manifest).await
        }
        AppManifest::ModelName(_) => {
            bail!("failed to retrieve manifest at `{:?}`", cmd.source)
        }
    }
}

async fn get_model_history(cmd: HistoryCommand) -> Result<VersionResponse> {
    let (lattice_prefix, client) =
        get_lattice_prefix_and_nats_client_from_cmd_opts(cmd.opts).await?;

    wash_lib::app::get_model_history(&client, lattice_prefix, &cmd.model_name).await
}

async fn get_model_details(cmd: GetCommand) -> Result<GetModelResponse> {
    let (lattice_prefix, client) =
        get_lattice_prefix_and_nats_client_from_cmd_opts(cmd.opts).await?;

    wash_lib::app::get_model_details(&client, lattice_prefix, &cmd.model_name, cmd.version).await
}

async fn delete_model_version(cmd: DeleteCommand) -> Result<DeleteModelResponse> {
    let (lattice_prefix, client) =
        get_lattice_prefix_and_nats_client_from_cmd_opts(cmd.opts).await?;

    wash_lib::app::delete_model_version(
        &client,
        lattice_prefix,
        &cmd.model_name,
        cmd.version,
        cmd.delete_all,
    )
    .await
}

async fn get_models(cmd: ListCommand) -> Result<Vec<ModelSummary>> {
    let (lattice_prefix, client) =
        get_lattice_prefix_and_nats_client_from_cmd_opts(cmd.opts).await?;
    wash_lib::app::get_models(&client, lattice_prefix).await
}

async fn get_lattice_prefix_and_nats_client_from_cmd_opts(
    opts: CliConnectionOpts,
) -> Result<(Option<String>, async_nats::client::Client)> {
    let connection_opts = <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(opts)?;
    let lattice_prefix = connection_opts.lattice_prefix.clone();
    let client = connection_opts.into_nats_client().await?;

    Ok((lattice_prefix, client))
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

#[cfg(test)]
mod test {
    use super::get_lattice_prefix_and_nats_client_from_cmd_opts;
    use anyhow::Result;
    use std::env;
    use wash_lib::{
        cli::CliConnectionOpts,
        config::{DEFAULT_CTX_DIR_NAME, DEFAULT_LATTICE_PREFIX, WASH_DIR},
        context::{fs::ContextDir, ContextManager, WashContext},
    };

    #[tokio::test]
    async fn test_lattice_prefix_and_nats_client_from_cmd_opts() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        env::set_current_dir(&tempdir)?;
        env::set_var("HOME", tempdir.path());

        // when opts.lattice_prefix.is_none() && opts.context.is_none() && user didn't set a default context, use the lattice_prefix from the preset default context...
        let opts = CliConnectionOpts::default();
        let (lattice_prefix, _) = get_lattice_prefix_and_nats_client_from_cmd_opts(opts).await?;
        assert_eq!(lattice_prefix, Some(DEFAULT_LATTICE_PREFIX.to_string()));

        // when opts.lattice_prefix.is_some() && opts.context.is_none(), use the specified lattice_prefix...
        let opts = CliConnectionOpts {
            lattice_prefix: Some("hal9000".to_string()),
            ..Default::default()
        };
        let (lattice_prefix, _) = get_lattice_prefix_and_nats_client_from_cmd_opts(opts).await?;
        assert_eq!(lattice_prefix, Some("hal9000".to_string()));

        let context_dir = ContextDir::new(
            tempdir
                .path()
                .join([WASH_DIR, DEFAULT_CTX_DIR_NAME].concat()),
        )?;

        // when opts.lattice_prefix.is_none() && opts.context.is_some(), use the lattice_prefix from the specified context...
        context_dir.save_context(&WashContext {
            name: "foo".to_string(),
            lattice_prefix: "iambatman".to_string(),
            ..Default::default()
        })?;
        let context_file = context_dir.get_context_path("foo")?.unwrap();
        let opts = CliConnectionOpts {
            context: Some(context_file.clone()),
            ..Default::default()
        };
        let (lattice_prefix, _) = get_lattice_prefix_and_nats_client_from_cmd_opts(opts).await?;
        assert_eq!(lattice_prefix, Some("iambatman".to_string()));

        // when opts.lattice_prefix.is_none() && opts.context.is_none(), use the lattice_prefix from the specified default context...
        context_dir.save_context(&WashContext {
            name: "bar".to_string(),
            lattice_prefix: "iamironman".to_string(),
            ..Default::default()
        })?;
        context_dir.set_default_context("bar")?;
        let opts = CliConnectionOpts::default();
        let (lattice_prefix, _) = get_lattice_prefix_and_nats_client_from_cmd_opts(opts).await?;
        assert_eq!(lattice_prefix, Some("iamironman".to_string()));

        Ok(())
    }
}

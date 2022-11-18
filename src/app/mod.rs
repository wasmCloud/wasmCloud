use std::{collections::HashMap, path::PathBuf, time::Duration};

use anyhow::{bail, Result};
use async_nats::Client;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::json;
use wash_lib::context::{
    fs::{load_context, ContextDir},
    ContextManager,
};

use crate::{
    appearance::spinner::Spinner,
    ctl::ConnectionOpts,
    ctx::{context_dir, ensure_host_config_context},
    util::{CommandOutput, OutputKind, DEFAULT_NATS_HOST, DEFAULT_NATS_PORT},
};

mod output;

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum AppCliCommand {
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
    #[clap(name = "del")]
    Delete(DeleteCommand),
    /// Puts a model version into the store
    #[clap(name = "put")]
    Put(PutCommand),
    /// Deploy an app (start a deployment monitor)
    #[clap(name = "deploy")]
    Deploy(DeployCommand),
    /// Undeploy an application (stop the deployment monitor)
    #[clap(name = "undeploy")]
    Undeploy(UndeployCommand),
}

#[derive(Args, Debug, Clone)]
pub(crate) struct ListCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,
}
#[derive(Args, Debug, Clone)]
pub(crate) struct UndeployCommand {
    /// Name of the app specification to undeploy
    #[clap(name = "name")]
    model_name: String,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct DeployCommand {
    /// Name of the app specification to deploy
    #[clap(name = "name")]
    model_name: String,

    /// Version of the app specification to deploy
    #[clap(name = "version")]
    version: String,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct DeleteCommand {
    /// Name of the app specification to delete
    #[clap(name = "name")]
    model_name: String,

    /// Version of the app specification to delete
    #[clap(name = "version")]
    version: String,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct PutCommand {
    /// Input filename (JSON or YAML) containing app specification
    source: PathBuf,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct GetCommand {
    /// The name of the app spec to retrieve
    #[clap(name = "name")]
    model_name: String,

    /// The version of the app spec to retrieve
    #[clap(name = "version")]
    version: String,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct HistoryCommand {
    /// The name of the app spec
    #[clap(name = "name")]
    model_name: String,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

pub(crate) async fn handle_command(
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
            let (raw, vetted) = write_model(results.clone())?;
            show_model_output(raw, vetted, results)
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

async fn undeploy_model(cmd: UndeployCommand) -> Result<bool> {
    let res = json_request(cmd.opts, &["undeploy", &cmd.model_name], json!({})).await?;

    Ok(res.is_some())
}

async fn deploy_model(cmd: DeployCommand) -> Result<bool> {
    let res = json_request(
        cmd.opts,
        &["deploy", &cmd.model_name],
        json!({
            "version": cmd.version
        }),
    )
    .await?;

    if let Some(v) = res {
        Ok(v["acknowledged"].as_bool().unwrap_or(false))
    } else {
        bail!("Failed to deploy application")
    }
}

async fn put_model(cmd: PutCommand) -> Result<PutReply> {
    let raw = std::fs::read_to_string(&cmd.source)?;
    let res = raw_request(cmd.opts, &["put"], raw.as_bytes()).await?;
    if let Some(v) = res {
        let r: PutReply = serde_json::from_value(v)?;
        Ok(r)
    } else {
        bail!("Failed to put app specification");
    }
}

async fn get_model_history(cmd: HistoryCommand) -> Result<Vec<ModelRevision>> {
    let res = json_request(cmd.opts, &["versions", &cmd.model_name], json!({})).await?;
    if let Some(v) = res {
        let revs: Vec<ModelRevision> = serde_json::from_value(v)?;
        Ok(revs)
    } else {
        bail!("Failed to get model history");
    }
}

async fn get_model_details(cmd: GetCommand) -> Result<ModelDetails> {
    let res = json_request(
        cmd.opts,
        &["get", &cmd.model_name],
        json!({
            "version": cmd.version
        }),
    )
    .await?;
    if let Some(v) = res {
        let md: ModelDetails = serde_json::from_value(v)?;
        Ok(md)
    } else {
        bail!("Failed to obtain reply from wadm");
    }
}

async fn delete_model_version(cmd: DeleteCommand) -> Result<bool> {
    let res = json_request(
        cmd.opts,
        &["del", &cmd.model_name],
        json!({
            "version": cmd.version
        }),
    )
    .await?;

    if res.is_none() {
        Ok(false)
    } else {
        Ok(true)
    }
}

async fn get_models(cmd: ListCommand) -> Result<Vec<ModelSummary>> {
    let res = json_request(cmd.opts, &["list"], json!({})).await?;

    if let Some(v) = res {
        let v: Vec<ModelSummary> = serde_json::from_value(v)?;
        Ok(v)
    } else {
        bail!("Failed to obtain reply from wadm");
    }
}

fn list_models_output(results: Vec<ModelSummary>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("apps".to_string(), json!(results));
    CommandOutput::new(output::list_models_table(results), map)
}

fn show_model_output(raw: PathBuf, vetted: PathBuf, md: ModelDetails) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("model".to_string(), json!(md));
    CommandOutput::new(output::show_model_details(raw, vetted), map)
}

fn show_put_results(results: PutReply) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("results".to_string(), json!(results));
    CommandOutput::new(
        format!(
            "App specification {} v{} stored",
            results.name, results.current_version
        ),
        map,
    )
}

fn show_undeploy_results(results: bool) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("results".to_string(), json!(results));
    CommandOutput::new(
        if results {
            "Undeploy request acknowledged"
        } else {
            "Undeploy request not acknowledged"
        },
        map,
    )
}

fn show_del_results(results: bool) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("deleted".to_string(), json!(results));
    CommandOutput::new(
        if results {
            "Model version deleted"
        } else {
            "Model version was not deleted"
        },
        map,
    )
}

fn show_deploy_results(results: bool) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("acknowledged".to_string(), json!(results));
    CommandOutput::new(
        if results {
            "App deployment request acknowledged".to_string()
        } else {
            "App deployment request failed".to_string()
        },
        map,
    )
}

fn show_model_history(results: Vec<ModelRevision>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("revisions".to_string(), json!(results));
    CommandOutput::new(output::list_revisions_table(results), map)
}

fn write_model(model: ModelDetails) -> Result<(PathBuf, PathBuf)> {
    let name = model.vetted["name"].as_str().unwrap_or("");
    let version = model.vetted["version"].as_str().unwrap_or("");
    let json_filename = format!("{}_v{}.json", name, version);
    let raw_filename = format!("{}_v{}.txt", name, version);

    let json_buf = PathBuf::from(json_filename);
    let raw_buf = PathBuf::from(raw_filename);
    let _ = std::fs::write(&json_buf, serde_json::to_vec(&model.vetted).unwrap());
    let _ = std::fs::write(&raw_buf, model.raw);

    Ok((raw_buf, json_buf))
}

async fn nats_client_from_opts(opts: ConnectionOpts) -> Result<(Client, Duration)> {
    // Attempt to load a context, falling back on the default if not supplied
    let ctx = if let Some(context) = opts.context {
        Some(load_context(context)?)
    } else if let Ok(ctx_dir) = context_dir(None) {
        let ctx_dir = ContextDir::new(ctx_dir)?;
        ensure_host_config_context(&ctx_dir)?;
        Some(ctx_dir.load_default_context()?)
    } else {
        None
    };

    let ctl_host = opts.ctl_host.unwrap_or_else(|| {
        ctx.as_ref()
            .map(|c| c.ctl_host.clone())
            .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string())
    });

    let ctl_port = opts.ctl_port.unwrap_or_else(|| {
        ctx.as_ref()
            .map(|c| c.ctl_port.to_string())
            .unwrap_or_else(|| DEFAULT_NATS_PORT.to_string())
    });

    let ctl_jwt = if opts.ctl_jwt.is_some() {
        opts.ctl_jwt
    } else {
        ctx.as_ref().map(|c| c.ctl_jwt.clone()).unwrap_or_default()
    };

    let ctl_seed = if opts.ctl_seed.is_some() {
        opts.ctl_seed
    } else {
        ctx.as_ref().map(|c| c.ctl_seed.clone()).unwrap_or_default()
    };

    let ctl_credsfile = if opts.ctl_credsfile.is_some() {
        opts.ctl_credsfile
    } else {
        ctx.as_ref()
            .map(|c| c.ctl_credsfile.clone())
            .unwrap_or_default()
    };

    let nc =
        crate::util::nats_client_from_opts(&ctl_host, &ctl_port, ctl_jwt, ctl_seed, ctl_credsfile)
            .await?;

    let timeout = Duration::from_millis(opts.timeout_ms);

    Ok((nc, timeout))
}

fn generate_topic(prefix: Option<String>, elements: &[&str]) -> String {
    let prefix = prefix.unwrap_or_else(|| "default".to_string());
    format!("wadm.api.{}.model.{}", prefix, elements.join("."))
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ModelSummary {
    pub name: String,
    pub version: String,
    pub description: String,
    pub deployment_status: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ModelRevision {
    pub version: String,
    pub created: String,
    pub deployed: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WadmEnvelope {
    pub result: String,
    pub message: Option<String>,
    pub data: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ModelDetails {
    pub raw: String,
    pub vetted: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct PutReply {
    pub current_version: String,
    pub name: String,
}

async fn raw_request(
    opts: ConnectionOpts,
    elements: &[&str],
    req: &[u8],
) -> Result<Option<serde_json::Value>> {
    let (nc, timeout) = nats_client_from_opts(opts.clone()).await?;
    let topic = generate_topic(opts.lattice_prefix, elements);

    match tokio::time::timeout(timeout, nc.request(topic, req.to_vec().into())).await {
        Ok(Ok(res)) => {
            let env: WadmEnvelope = serde_json::from_slice(&res.payload)?;
            if env.result == "success" {
                Ok(env.data)
            } else {
                bail!("{}", env.message.unwrap_or_default())
            }
        }
        Ok(Err(e)) => bail!("Error making message request: {}", e),
        Err(e) => bail!("Request timed out:  {}", e),
    }
}

async fn json_request(
    opts: ConnectionOpts,
    elements: &[&str],
    req: serde_json::Value,
) -> Result<Option<serde_json::Value>> {
    let msg = serde_json::to_vec(&req)?;
    raw_request(opts, elements, &msg).await
}

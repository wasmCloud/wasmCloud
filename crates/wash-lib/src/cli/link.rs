use anyhow::{Context, Result};
use clap::Parser;
use wasmcloud_control_interface::{CtlResponse, InterfaceLinkDefinition};

use crate::{cli::CliConnectionOpts, common::boxed_err_to_anyhow, config::WashConnectionOptions};

#[derive(Parser, Debug, Clone)]
pub struct LinkDelCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Public key ID or name of actor to match on. If an actor name is given and matches multiple
    /// actors, an error will be returned with a list of matching actors and their IDs.
    #[clap(name = "source-id")]
    pub source_id: String,

    /// Link name, defaults to "default"
    #[clap(short = 'l', long = "link-name")]
    pub link_name: Option<String>,

    /// WIT namespace of the link
    #[clap(short = 'n', long = "wit-namespace")]
    pub wit_namespace: String,

    /// WIT package of the link
    #[clap(short = 'p', long = "wit-package")]
    pub wit_package: String,
}

#[derive(Parser, Debug, Clone)]
#[clap(
    override_usage = "wash ctl link put --link-name <LINK_NAME> [OPTIONS] <actor-id-or-name> <provider-id-or-name> <contract-id> [values]..."
)]
pub struct LinkPutCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    #[clap(name = "source-id")]
    pub source_id: String,

    #[clap(name = "target")]
    pub target: String,

    #[clap(name = "wit-namespace")]
    pub wit_namespace: String,

    #[clap(name = "wit-package")]
    pub wit_package: String,

    #[clap(long = "interface")]
    pub interfaces: Vec<String>,

    #[clap(long = "source_config")]
    pub source_config: Vec<String>,

    #[clap(long = "target_config")]
    pub target_config: Vec<String>,

    /// Link name, defaults to "default"
    #[clap(short = 'l', long = "link-name")]
    pub link_name: Option<String>,
}

#[derive(Parser, Debug, Clone)]
pub struct LinkQueryCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,
}

#[derive(Debug, Clone, Parser)]
pub enum LinkCommand {
    /// Query established links
    #[clap(name = "query")]
    Query(LinkQueryCommand),

    /// Establish a link definition
    #[clap(name = "put")]
    Put(LinkPutCommand),

    /// Delete a link definition
    #[clap(name = "del")]
    Del(LinkDelCommand),
}

/// Query links for a given Wash instance
///
/// # Arguments
///
/// * `wco` - Options for connecting to wash
///
/// # Examples
///
/// ```no_run
/// let ack = query_links(WashConnectionOptions::default()).await?;
/// assert_eq!(ack.accepted, true);
/// ```
pub async fn get_links(wco: WashConnectionOptions) -> Result<Vec<InterfaceLinkDefinition>> {
    wco.into_ctl_client(None)
        .await?
        .get_links()
        .await
        .map(|ctl| ctl.response.unwrap_or_default())
        .map_err(boxed_err_to_anyhow)
}

/// Delete a single link
///
/// # Arguments
///
/// * `wco` - Options for connecting to wash
/// * `source_id` - The ID of the source attached to the link
/// * `link_name` - The link name of the link ('default')
/// * `wit_namespace` - The WIT namespace of the link
/// * `wit_package` - The WIT package of the link
///
/// # Examples
///
/// ```no_run
/// let ack = delete_link(
///   WashConnectionOptions::default(),
///   "httpserver",
///   "default",
///   "wasi",
///   "http",
/// ).await?;
/// assert_eq!(ack.accepted, true);
/// ```
pub async fn delete_link(
    wco: WashConnectionOptions,
    source_id: &str,
    link_name: &str,
    wit_namespace: &str,
    wit_package: &str,
) -> Result<CtlResponse<()>> {
    let ctl_client = wco.into_ctl_client(None).await?;
    ctl_client
        .delete_link(source_id, link_name, wit_namespace, wit_package)
        .await
        .map_err(boxed_err_to_anyhow)
        .with_context(|| {
            format!(
                "Failed to remove link from {source_id} on {wit_namespace}:{wit_package} with link name {link_name}",
            )
        })
}

/// Put a new link
///
/// # Arguments
///
/// * `wco` - Options for connecting to wash
/// * `link` - The [wasmcloud_control_interface::InterfaceLinkDefinition] to create
///
/// # Examples
///
/// ```no_run
/// let ack = delete_link(
///   WashConnectionOptions::default(),
///   InterfaceLinkDefinition {
///    source_id: "httpserver".to_string(),
///    target: "echo".to_string(), // wasmcloud.azurecr.io/echo:0.3.8
///    wit_namespace: "wasi".to_string(),
///    wit_package: "http".to_string(),
///    link_name: "default".to_string(),
///    interfaces: vec!["incoming-handler".to_string()],
///    source_config: vec![],
///    target_config: vec![],
///   }
/// ).await?;
/// assert_eq!(ack.accepted, true);
/// ```
pub async fn put_link(
    wco: WashConnectionOptions,
    link: InterfaceLinkDefinition,
) -> Result<CtlResponse<()>> {
    let ctl_client = wco.into_ctl_client(None).await?;
    ctl_client
        .put_link(link.clone())
        .await
        .map_err(boxed_err_to_anyhow)
        .with_context(|| {
            format!(
                "Failed to create link between {} and {} on {}:{}/{:?}. Link name: {}",
                link.source_id,
                link.target,
                link.wit_namespace,
                link.wit_package,
                link.interfaces,
                link.name
            )
        })
}

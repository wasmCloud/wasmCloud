use anyhow::{Context, Result};
use clap::Parser;
use wasmcloud_control_interface::{CtlResponse, Link};

use crate::{cli::CliConnectionOpts, common::boxed_err_to_anyhow, config::WashConnectionOptions};

use super::validate_component_id;

#[derive(Parser, Debug, Clone)]
pub struct LinkDelCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Component ID or name of the source of the link.
    #[clap(name = "source-id", value_parser = validate_component_id)]
    pub source_id: String,

    /// Link name, defaults to "default"
    #[clap(short = 'l', long = "link-name")]
    pub link_name: Option<String>,

    /// WIT namespace of the link
    #[clap(name = "wit-namespace")]
    pub wit_namespace: String,

    /// WIT package of the link
    #[clap(name = "wit-package")]
    pub wit_package: String,
}

#[derive(Parser, Debug, Clone)]
pub struct LinkPutCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// The ID of the component to link from
    #[clap(name = "source-id", value_parser = validate_component_id)]
    pub source_id: String,

    /// The ID of the component to link to
    #[clap(name = "target", value_parser = validate_component_id)]
    pub target: String,

    /// The WIT namespace of the link, e.g. "wasi" in "wasi:http/incoming-handler"
    #[clap(name = "wit-namespace")]
    pub wit_namespace: String,

    /// The WIT package of the link, e.g. "http" in "wasi:http/incoming-handler"
    #[clap(name = "wit-package")]
    pub wit_package: String,

    /// The interface of the link, e.g. "incoming-handler" in "wasi:http/incoming-handler"
    #[clap(long = "interface", alias = "interfaces", required = true)]
    pub interfaces: Vec<String>,

    /// List of named configuration to make available to the source
    #[clap(long = "source-config")]
    pub source_config: Vec<String>,

    /// List of named configuration to make available to the target
    #[clap(long = "target-config")]
    pub target_config: Vec<String>,

    /// Link name, defaults to "default". Used for scenarios where a single source
    /// may have multiple links to the same target, or different targets with the same
    /// WIT namespace, package, and interface.
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
    /// Query all links, same as `wash get links`
    #[clap(name = "query", alias = "get")]
    Query(LinkQueryCommand),

    /// Put a link from a source to a target on a given WIT interface
    #[clap(name = "put")]
    Put(LinkPutCommand),

    /// Delete a link
    #[clap(name = "del", alias = "delete")]
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
pub async fn get_links(wco: WashConnectionOptions) -> Result<Vec<Link>> {
    wco.into_ctl_client(None)
        .await?
        .get_links()
        .await
        .map(|ctl| ctl.into_data().unwrap_or_default())
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
/// * `link` - The [`wasmcloud_control_interface::InterfaceLinkDefinition`] to create
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
pub async fn put_link(wco: WashConnectionOptions, link: Link) -> Result<CtlResponse<()>> {
    let ctl_client = wco.into_ctl_client(None).await?;
    ctl_client
        .put_link(link.clone())
        .await
        .map_err(boxed_err_to_anyhow)
        .with_context(|| {
            format!(
                "Failed to create link between {} and {} on {}:{}/{:?}. Link name: {}",
                link.source_id(),
                link.target(),
                link.wit_namespace(),
                link.wit_package(),
                link.interfaces(),
                link.name()
            )
        })
}

use anyhow::{Context, Result};
use clap::Parser;
use wasmcloud_control_interface::LinkDefinition;

use crate::{
    cli::{labels_vec_to_hashmap, CliConnectionOpts},
    common::boxed_err_to_anyhow,
    config::WashConnectionOptions,
    id::{ModuleId, ServiceId},
};

#[derive(Parser, Debug, Clone)]
pub struct LinkDelCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Public key ID of actor
    #[clap(name = "actor-id", value_parser)]
    pub actor_id: ModuleId,

    /// Capability contract ID between actor and provider
    #[clap(name = "contract-id")]
    pub contract_id: String,

    /// Link name, defaults to "default"
    #[clap(short = 'l', long = "link-name")]
    pub link_name: Option<String>,
}

#[derive(Parser, Debug, Clone)]
#[clap(
    override_usage = "wash ctl link put --link-name <LINK_NAME> [OPTIONS] <actor-id> <provider-id> <contract-id> [values]..."
)]
pub struct LinkPutCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Public key ID of actor
    #[clap(name = "actor-id", value_parser)]
    pub actor_id: ModuleId,

    /// Public key ID of provider
    #[clap(name = "provider-id", value_parser)]
    pub provider_id: ServiceId,

    /// Capability contract ID between actor and provider
    #[clap(name = "contract-id")]
    pub contract_id: String,

    /// Link name, defaults to "default"
    #[clap(short = 'l', long = "link-name")]
    pub link_name: Option<String>,

    /// Environment values to provide alongside link
    #[clap(name = "values")]
    pub values: Vec<String>,
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
pub async fn query_links(wco: WashConnectionOptions) -> Result<Vec<LinkDefinition>> {
    wco.into_ctl_client(None)
        .await?
        .query_links()
        .await
        .map_err(boxed_err_to_anyhow)
}

/// Delete a single link
///
/// # Arguments
///
/// * `wco` - Options for connecting to wash
/// * `actor_id` - The ID of the actor attached to the link
/// * `contract_id` - The contract ID of the link
/// * `link_name` - The link name of the link ('default')
///
/// # Examples
///
/// ```no_run
/// let ack = delete_link(
///   WashConnectionOptions::default(),
///   "wasmcloud:httpserver",
///   "MBCFOPM6JW2APJLXJD3Z5O4CN7CPYJ2B4FTKLJUR5YR5MITIU7HD3WD5", // wasmcloud.azurecr.io/echo:0.3.8
///   "default",
/// ).await?;
/// assert_eq!(ack.accepted, true);
/// ```
pub async fn delete_link(
    wco: WashConnectionOptions,
    contract_id: &str,
    actor_id: &ModuleId,
    link_name: &str,
) -> Result<()> {
    wco.into_ctl_client(None)
        .await?
        .remove_link(actor_id, contract_id, link_name)
        .await
        .map_err(boxed_err_to_anyhow)
        .with_context(|| {
            format!(
                "Failed to remove link between {} and {} with link name {}",
                actor_id, contract_id, link_name
            )
        })
}

/// Create ("put") a new link
///
/// # Arguments
///
/// * `wco` - Options for connecting to wash
/// * `contract_id` - The contract ID of the link
/// * `actor_id` - The ID of the actor attached to the link
/// * `provider_id` - The ID of the provider attached to the link
/// * `link_name` - The link name of the link ('default')
///
/// # Examples
///
/// ```no_run
/// let ack = delete_link(
///   WashConnectionOptions::default(),
///   "wasmcloud:httpserver",
///   "MBCFOPM6JW2APJLXJD3Z5O4CN7CPYJ2B4FTKLJUR5YR5MITIU7HD3WD5", // wasmcloud.azurecr.io/echo:0.3.8
///   "VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M", // wasmcloud.azurecr.io/httpserver:0.17.0
///   "default",
///   vec!["KEY", "value"],
/// ).await?;
/// assert_eq!(ack.accepted, true);
/// ```
pub async fn create_link(
    wco: WashConnectionOptions,
    contract_id: &str,
    actor_id: &ModuleId,
    provider_id: &ServiceId,
    link_name: &str,
    link_values: &Vec<String>,
) -> Result<()> {
    wco.into_ctl_client(None)
        .await?
        .advertise_link(
            actor_id,
            provider_id,
            contract_id,
            link_name,
            labels_vec_to_hashmap(link_values.clone())?,
        )
        .await
        .map_err(boxed_err_to_anyhow)
        .with_context(|| {
            format!(
                "Failed to create link between {} and {} with contract {}. Link name: {}, values: {:?}",
                actor_id, provider_id, contract_id, link_name, &link_values
            )
        })
}

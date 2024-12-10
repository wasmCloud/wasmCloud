use std::fmt::{Debug, Display};
use std::path::Path;
use std::process::Stdio;
use std::str::FromStr;

use anyhow::{anyhow, bail, Result};
use tokio::process::Command;

use anyhow::Context;
use tracing::error;
use wasmcloud_control_interface::HostInventory;

use crate::id::{ModuleId, ServerId, ServiceId};

/// Default path to the `git` command (assumes it exists on PATH)
const DEFAULT_GIT_PATH: &str = "git";

const CLAIMS_CALL_ALIAS: &str = "call_alias";
pub(crate) const CLAIMS_NAME: &str = "name";
pub(crate) const CLAIMS_SUBJECT: &str = "sub";

/// Converts error from Send + Sync error to standard anyhow error
pub(crate) fn boxed_err_to_anyhow(e: Box<dyn ::std::error::Error + Send + Sync>) -> anyhow::Error {
    anyhow::anyhow!(e)
}

#[derive(Debug, thiserror::Error)]
pub enum FindIdError {
    /// No matches were found
    #[error("No matches found with the provided search term")]
    NoMatches,
    /// Multiple matches were found. The vector contains the list of components or providers that
    /// matched
    #[error("Multiple matches found with the provided search term: {0:?}")]
    MultipleMatches(Vec<Match>),
    #[error(transparent)]
    Error(#[from] anyhow::Error),
}

/// Represents a single match against a search term
#[derive(Clone)]
pub struct Match {
    pub id: String,
    pub friendly_name: Option<String>,
}

impl Display for Match {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(friendly_name) = &self.friendly_name {
            write!(f, "{} ({friendly_name})", self.id)
        } else {
            write!(f, "{}", self.id)
        }
    }
}

impl Debug for Match {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self, f)
    }
}

/// Whether or not to use a command group to manage unix/windows signal delivery
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum CommandGroupUsage {
    /// Use the parent command group
    #[default]
    UseParent,
    /// Create a new command group (using this option prevents signals from being delivered)
    /// automatically to subprocesses
    CreateNew,
}

/// Given a string, attempts to resolve a component ID. Returning the component ID and an optional friendly
/// name
///
/// If the string is a valid component ID, it will be returned unchanged. If it is not an ID, it will
/// attempt to resolve an ID in the following order:
///
/// 1. The value matches the prefix of the ID of a component
/// 2. The value is contained in the call alias of a component
/// 3. The value is contained in the name field of a component
///
/// If more than one matches, then an error will be returned indicating the options to choose from
pub async fn find_component_id(
    value: &str,
    ctl_client: &wasmcloud_control_interface::Client,
) -> Result<(ModuleId, Option<String>), FindIdError> {
    find_id_matches(value, ctl_client).await
}

/// Given a string, attempts to resolve a provider ID. Returning the provider ID and an optional
/// friendly name
///
/// If the string is a valid provider ID, it will be returned unchanged. If it is not an ID, it will
/// attempt to resolve an ID in the following order:
///
/// 1. The value matches the prefix of the ID of a provider
/// 2. The value is contained in the name field of a provider
///
/// If more than one matches, then an error will be returned indicating the options to choose from
pub async fn find_provider_id(
    value: &str,
    ctl_client: &wasmcloud_control_interface::Client,
) -> Result<(ServiceId, Option<String>), FindIdError> {
    find_id_matches(value, ctl_client).await
}

async fn find_id_matches<T: FromStr + ToString + Display>(
    value: &str,
    ctl_client: &wasmcloud_control_interface::Client,
) -> Result<(T, Option<String>), FindIdError> {
    if let Ok(id) = T::from_str(value) {
        return Ok((id, None));
    }
    // Case insensitive searching here to make things nicer
    let value = value.to_lowercase();
    // If it wasn't an ID, get the claims
    let ctl_response = ctl_client
        .get_claims()
        .await
        .map_err(boxed_err_to_anyhow)
        .context("unable to get claims for lookup")?;
    let Some(claims) = ctl_response.into_data() else {
        error!("received claims response from control interface but no claims were present in the response");
        return Err(FindIdError::NoMatches);
    };

    let all_matches = claims
        .iter()
        .filter_map(|v| {
            let id_str = v
                .get(CLAIMS_SUBJECT)
                .map(String::as_str)
                .unwrap_or_default();
            // If it doesn't parse to our type, just skip
            let id = match T::from_str(id_str) {
                Ok(id) => id,
                Err(_) => return None,
            };
            (id_str.to_lowercase().starts_with(&value)
                || v.get(CLAIMS_CALL_ALIAS)
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default()
                    .contains(&value)
                || v.get(CLAIMS_NAME)
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_default()
                    .contains(&value))
            .then(|| (id, v.get(CLAIMS_NAME).map(ToString::to_string)))
        })
        .collect::<Vec<_>>();

    if all_matches.is_empty() {
        Err(FindIdError::NoMatches)
    } else if all_matches.len() > 1 {
        Err(FindIdError::MultipleMatches(
            all_matches
                .into_iter()
                .map(|(id, friendly_name)| Match {
                    id: id.to_string(),
                    friendly_name,
                })
                .collect(),
        ))
    } else {
        // SAFETY: We know we have exactly one match at this point
        Ok(all_matches.into_iter().next().unwrap())
    }
}

/// Given a string, attempts to resolve a host ID. Returning the host ID and its friendly name.
///
/// If the string is a valid host ID, it will be returned unchanged. If it is not an ID, it will
/// attempt to resolve an ID in the following order:
///
/// 1. The value matches the prefix of the ID of a host
/// 2. The value is contained in the friendly name field of a host
///
/// If more than one matches, then an error will be returned indicating the options to choose from
pub async fn find_host_id(
    value: &str,
    ctl_client: &wasmcloud_control_interface::Client,
) -> Result<(ServerId, String), FindIdError> {
    if let Ok(id) = ServerId::from_str(value) {
        return Ok((id, String::new()));
    }

    // Case insensitive searching here to make things nicer
    let value = value.to_lowercase();

    let hosts = ctl_client
        .get_hosts()
        .await
        .map_err(boxed_err_to_anyhow)
        .context("unable to fetch hosts for lookup")?;

    let all_matches = hosts
        .into_iter()
        .filter_map(|h| h.into_data())
        .filter_map(|h| {
            if h.id().to_lowercase().starts_with(&value)
                || h.friendly_name().to_lowercase().contains(&value)
            {
                ServerId::from_str(h.id())
                    .ok()
                    .map(|id| (id, h.friendly_name().to_string()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if all_matches.is_empty() {
        Err(FindIdError::NoMatches)
    } else if all_matches.len() > 1 {
        Err(FindIdError::MultipleMatches(
            all_matches
                .into_iter()
                .map(|(id, friendly_name)| Match {
                    id: id.to_string(),
                    friendly_name: Some(friendly_name.to_string()),
                })
                .collect(),
        ))
    } else {
        // SAFETY: We know we have exactly one match at this point
        Ok(all_matches.into_iter().next().unwrap())
    }
}

pub async fn get_all_inventories(
    client: &wasmcloud_control_interface::Client,
) -> anyhow::Result<Vec<HostInventory>> {
    let hosts = client.get_hosts().await.map_err(boxed_err_to_anyhow)?;
    let host_ids = match hosts.len() {
        0 => return Ok(Vec::with_capacity(0)),
        _ => hosts
            .into_iter()
            .filter_map(|h| h.into_data().map(|h| h.id().to_string())),
    };

    let futs =
        host_ids
            .map(|host_id| (client.clone(), host_id))
            .map(|(client, host_id)| async move {
                client
                    .get_host_inventory(&host_id)
                    .await
                    .map(|inventory| inventory.into_data())
                    .map_err(boxed_err_to_anyhow)
            });
    futures::future::join_all(futs)
        .await
        .into_iter()
        .filter_map(Result::transpose)
        .collect::<anyhow::Result<Vec<HostInventory>>>()
}

/// Clone a git repository
pub async fn clone_git_repo(
    git_cmd: Option<String>,
    tmp_dir: impl AsRef<Path>,
    repo_url: String,
    sub_folder: Option<String>,
    repo_branch: Option<String>,
) -> Result<()> {
    let git_cmd = git_cmd.unwrap_or_else(|| DEFAULT_GIT_PATH.into());
    let tmp_dir = tmp_dir.as_ref();
    let cwd =
        std::env::current_dir().map_err(|e| anyhow!("could not get current directory: {}", e))?;
    std::env::set_current_dir(tmp_dir)
        .map_err(|e| anyhow!("could not cd to tmp dir {}: {}", tmp_dir.display(), e))?;
    // for convenience, allow omission of prefix 'https://' or 'https://github.com'
    let repo_url = {
        if repo_url.starts_with("http://")
            || repo_url.starts_with("https://")
            || repo_url.starts_with("git+ssh://")
        {
            repo_url
        } else if repo_url.starts_with("git+https://")
            || repo_url.starts_with("git+http://")
            || repo_url.starts_with("git+ssh")
        {
            repo_url.replace("git+", "")
        } else if repo_url.starts_with("github.com/") {
            format!("https://{}", &repo_url)
        } else {
            format!("https://github.com/{}", repo_url.trim_start_matches('/'))
        }
    };

    // Build args for git clone command
    let mut args = vec!["clone", &repo_url, "--depth", "1", "."];
    if let Some(ref branch) = repo_branch {
        args.push("--no-checkout");
        args.push("--branch");
        args.push(branch);
    }

    let clone_cmd_out = Command::new(&git_cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?
        .wait_with_output()
        .await?;
    if !clone_cmd_out.status.success() {
        bail!(
            "git clone error: {}",
            String::from_utf8_lossy(&clone_cmd_out.stderr)
        );
    }

    if let Some(sub_folder) = sub_folder {
        let checkout_cmd_out = Command::new(&git_cmd)
            .args(["sparse-checkout", "set", &sub_folder])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()
            .await?;
        if !checkout_cmd_out.status.success() {
            bail!(
                "git sparse-checkout set error: {}",
                String::from_utf8_lossy(&checkout_cmd_out.stderr)
            );
        }
    }

    // Checkout the branch, if provided
    if let Some(branch) = repo_branch {
        let checkout_cmd_out = Command::new(&git_cmd)
            .args(["checkout", &branch])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()
            .await?;
        if !checkout_cmd_out.status.success() {
            bail!(
                "git checkout error: {}",
                String::from_utf8_lossy(&checkout_cmd_out.stderr)
            );
        }
    }

    std::env::set_current_dir(cwd)?;
    Ok(())
}

use anyhow::anyhow;
use clap_complete::engine::CompletionCandidate;
use std::ffi::OsStr;
use std::fs::read_dir;
use std::path::Path;
use tokio::runtime::Handle;
use tokio::task;

use crate::cli::cli::{CliCommand, create_cli, get_connection_opts_from_cli};
use crate::lib::cli::CliConnectionOpts;
use crate::lib::config::WashConnectionOptions;
use wadm_types::api::StatusType;

use super::AppCliCommand;

// Used as input for app_name completer
#[derive(PartialEq)]
enum DesiredAppStatusType {
    All,
    Deployed,
    Undeployed,
}

// Used to identify the app name already specified in order to retrieve candidates for dynamic autocompletion
// of the respective subcommand.
fn get_app_name_from_cli() -> anyhow::Result<Option<String>> {
    match create_cli() {
        Some(CliCommand::App(AppCliCommand::Delete(cmd))) => Ok(cmd.app_name),
        Some(CliCommand::App(AppCliCommand::Deploy(cmd))) => Ok(cmd.app_name),
        Some(CliCommand::App(AppCliCommand::Get(cmd))) => Ok(cmd.app_name),
        _ => Err(anyhow!("Command did not match any expected patterns")),
    }
}

pub fn app_name_completer(_current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    task::block_in_place(
        || Handle::current()
            .block_on(get_declared_apps(DesiredAppStatusType::All))
    ).unwrap_or(vec![])
}

pub fn deployed_app_name_completer(_current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    task::block_in_place(
        || Handle::current()
            .block_on(get_declared_apps(DesiredAppStatusType::Deployed))
    ).unwrap_or(vec![])
}

pub fn undeployed_app_name_completer(_current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    task::block_in_place(
        || Handle::current()
            .block_on(get_declared_apps(DesiredAppStatusType::Undeployed))
    ).unwrap_or(vec![])
}

// Getting the names of declared applications from WADM and filter them as potential candidates
async fn get_declared_apps(status: DesiredAppStatusType) -> anyhow::Result<Vec<CompletionCandidate>> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(get_connection_opts_from_cli()?)?;
    let lattice = Some(connection_opts.get_lattice());
    let client = connection_opts.into_nats_client().await?;
    let models = crate::lib::app::get_models(&client, lattice).await?;

    let candidates: Vec<CompletionCandidate> = models
        .iter()
        .filter(|ms| {
            match status {
                DesiredAppStatusType::All => true,
                DesiredAppStatusType::Deployed => {StatusType::Deployed == ms.detailed_status.info.status_type},
                DesiredAppStatusType::Undeployed => {StatusType::Undeployed == ms.detailed_status.info.status_type},
            }
        })
        .map(|ms| {
            CompletionCandidate::new(&ms.name)
        })
        .collect();

    Ok(candidates)
}

pub fn version_completer(_current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    task::block_in_place(|| Handle::current().block_on(get_declared_versions())).unwrap_or(vec![])
}

// Getting the versions of declared applications from WADM as potential candidates
async fn get_declared_versions() -> anyhow::Result<Vec<CompletionCandidate>> {
    let connection_opts =
        <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(get_connection_opts_from_cli()?)?;
    let lattice = Some(connection_opts.get_lattice());
    let client = connection_opts.into_nats_client().await?;
    let app_name = get_app_name_from_cli()?.unwrap_or("".to_string());
    let versions = crate::lib::app::get_model_history(&client, lattice, &app_name).await?;

    let candidates: Vec<CompletionCandidate> = versions
        .iter()
        .map(|info| CompletionCandidate::new(&info.version))
        .collect();

    Ok(candidates)
}

pub fn path_completer(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    task::block_in_place(|| Handle::current().block_on(get_local_manifests(current.to_str().unwrap_or_default()))).unwrap_or(vec![])
}

// TODO: Currently, if no other candidate exists clap will automatically append a space
// at the end of the arg, see: https://github.com/clap-rs/clap/issues/5587
// Once this is configurable, we need to prevent the trailing space for directories.
async fn get_local_manifests(path: &str) -> anyhow::Result<Vec<CompletionCandidate>> {

    // Setting path for lookup from requested, potentially incomplete path
    let p= if path.ends_with("/") {
        // `path` is a valid, relative or absolut path
        // (e.g. <parent_dir>/manifests/, ./manifests/, manifests/, ...).
        // Therefore it can be directly used for identifying candidates.
        Path::new(path)
    } else if path.contains("/") {
        // `path` is an incomplete, relative or absolut path as str
        // (e.g. <parent_dir>/mani, ./, ./mani, ...).
        // Therefore the parent directory must be used for identifying candidates.
        Path::new(path).parent().unwrap_or(Path::new("./"))
    } else {
        // `path` is an incomplete, relative path as str (e.g. manife).
        // Therefore we use the current directory for identifying candidates.
        Path::new("./")
    };

    let entries = read_dir(p)?;
    let candidates: Vec<CompletionCandidate> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            use std::os::unix::ffi::OsStrExt;
            // We want to return non-hidden directories
            if e.file_type().unwrap().is_dir() && e.file_name().as_os_str().as_bytes()[0] != b'.' {
                true
            } else {
                // Otherwise we want to return non-hidden yaml files
                e.path().extension() == Some(OsStr::from_bytes(b"yaml")) && e.file_name().as_os_str().as_bytes()[0] != b'.'
            }
        })
        .map(|e| {
            // Removing prefix for current directory if needed.
            // This is necessary because we are using that prefix as the default path
            // for incomplete, relative path (see above).
            let p = if e.path().starts_with("./") && !path.starts_with("./") {
                let p = e.path().clone();
                p.strip_prefix("./").unwrap_or(p.as_path()).to_path_buf()
            } else {
                e.path()
            };
            if e.file_type().unwrap().is_dir() {
                // Adding a trailing "/" so that for the next candidate identification
                // the current path can be used as is - see the initial matching of this function
                CompletionCandidate::new(p.join(""))
            } else {
                CompletionCandidate::new(p)
            }
        })
        .collect();

    Ok(candidates)
}

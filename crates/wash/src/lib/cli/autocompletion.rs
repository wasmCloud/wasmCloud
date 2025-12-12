use anyhow::Context;
use clap_complete::engine::CompletionCandidate;
use tokio::runtime::Handle;
use tokio::task;

use crate::cli::cli::get_connection_opts_from_cli;
use crate::lib::{
    common::get_all_inventories,
    config::WashConnectionOptions,
};

pub fn component_id_completer(_current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    task::block_in_place(|| Handle::current().block_on(get_component_list())).unwrap_or(vec![])
}

async fn get_component_list() -> anyhow::Result<Vec<CompletionCandidate>> {
    let connection_opts = get_connection_opts_from_cli()?;
    let wco: WashConnectionOptions = connection_opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;
    let inventories = get_all_inventories(&client)
        .await
        .context("unable to fetch all inventory")?;

    let candidates: Vec<CompletionCandidate> = inventories
        .iter()
        .flat_map(|i| {
            i.components
                .iter()
                .map(|cd| CompletionCandidate::new(cd.id.clone()))
                .chain(
                    i.providers
                        .iter()
                        .map(|pd| CompletionCandidate::new(pd.id.clone()))
                )
        })
        .collect();

    Ok(candidates)
}

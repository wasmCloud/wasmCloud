//! Utilities for managing lattice links

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use nkeys::KeyPair;

use wascap::jwt;

pub async fn assert_advertise_link(
    client: impl Into<&wasmcloud_control_interface::Client>,
    actor_claims: impl Into<&jwt::Claims<jwt::Actor>>,
    provider_key: impl Into<&KeyPair>,
    contract_id: impl AsRef<str>,
    link_name: impl AsRef<str>,
    values: HashMap<String, String>,
) -> Result<()> {
    let client = client.into();
    let actor_claims = actor_claims.into();
    let provider_key = provider_key.into();
    let contract_id = contract_id.as_ref();
    let link_name = link_name.as_ref();
    client
        .advertise_link(
            &actor_claims.subject,
            &provider_key.public_key(),
            contract_id,
            link_name,
            values,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to advertise link"))?;
    Ok(())
}

pub async fn assert_remove_link(
    client: impl Into<&wasmcloud_control_interface::Client>,
    actor_claims: impl Into<&jwt::Claims<jwt::Actor>>,
    contract_id: impl AsRef<str>,
    link_name: impl AsRef<str>,
) -> Result<()> {
    let client = client.into();
    let actor_claims = actor_claims.into();
    let contract_id = contract_id.as_ref();
    let link_name = link_name.as_ref();
    client
        .remove_link(&actor_claims.subject, contract_id, link_name)
        .await
        .map_err(|e| anyhow!(e).context("failed to remove link"))?;
    Ok(())
}

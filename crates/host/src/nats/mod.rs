//! NATS implementations of wasmCloud [crate::wasmbus::Host] extension traits

use std::{sync::Arc, time::Duration};

use anyhow::{bail, Context as _};
use async_nats::jetstream::kv::Store;
use nkeys::KeyPair;
use tracing::{info, instrument};

use crate::workload_identity::{
    setup_workload_identity_nats_connect_options, WorkloadIdentityConfig,
};

/// Helper module for building a wasmCloud host with NATS as the primary transport.
pub mod builder;

/// NATS implementation of the wasmCloud control interface
pub mod ctl;

/// NATS implementation of the wasmCloud [crate::event::EventPublisher] extension trait,
/// sending events to the NATS message bus with a CloudEvents payload envelope.
pub mod event;

/// NATS implementation of the wasmCloud [crate::policy::PolicyManager] trait
pub mod policy;

/// NATS implementation of the wasmCloud [crate::wasmbus::providers::ProviderManager] extension trait
/// which sends provider commands over the NATS message bus.
pub mod provider;

/// NATS implementation of the [crate::secrets::SecretsManager] extension trait
/// for fetching encrypted secrets from a secret store.
pub mod secrets;

/// NATS implementation of the wasmCloud [crate::store::StoreManager] extension trait
/// using JetStream as a backing store.
pub mod store;

/// Given the NATS address, authentication jwt, seed, tls requirement and optional request timeout,
/// attempt to establish connection.
///
/// This function should be used to create a NATS client for Host communication, for non-host NATS
/// clients we recommend using async-nats directly.
///
/// # Errors
///
/// Returns an error if:
/// - Only one of JWT or seed is specified, as we cannot authenticate with only one of them
/// - Connection fails
pub async fn connect_nats(
    addr: impl async_nats::ToServerAddrs,
    jwt: Option<&String>,
    key: Option<Arc<KeyPair>>,
    require_tls: bool,
    request_timeout: Option<Duration>,
    workload_identity_config: Option<WorkloadIdentityConfig>,
) -> anyhow::Result<async_nats::Client> {
    let opts = match (jwt, key, workload_identity_config) {
        (Some(jwt), Some(key), None) => {
            async_nats::ConnectOptions::with_jwt(jwt.to_string(), move |nonce| {
                let key = key.clone();
                async move { key.sign(&nonce).map_err(async_nats::AuthError::new) }
            })
            .name("wasmbus")
        }
        (Some(_), None, _) | (None, Some(_), _) => {
            bail!("cannot authenticate if only one of jwt or seed is specified")
        }
        (jwt, key, Some(wid_cfg)) => {
            setup_workload_identity_nats_connect_options(jwt, key, wid_cfg).await?
        }
        _ => async_nats::ConnectOptions::new().name("wasmbus"),
    };
    let opts = if let Some(timeout) = request_timeout {
        opts.request_timeout(Some(timeout))
    } else {
        opts
    };
    let opts = opts.require_tls(require_tls);
    opts.connect(addr)
        .await
        .context("failed to connect to NATS")
}

#[instrument(level = "debug", skip_all)]
pub(crate) async fn create_bucket(
    jetstream: &async_nats::jetstream::Context,
    bucket: &str,
) -> anyhow::Result<Store> {
    // Don't create the bucket if it already exists
    if let Ok(store) = jetstream.get_key_value(bucket).await {
        info!(%bucket, "bucket already exists. Skipping creation.");
        return Ok(store);
    }

    match jetstream
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: bucket.to_string(),
            ..Default::default()
        })
        .await
    {
        Ok(store) => {
            info!(%bucket, "created bucket with 1 replica");
            Ok(store)
        }
        Err(err) => {
            Err(anyhow::anyhow!(err).context(format!("failed to create bucket '{bucket}'")))
        }
    }
}

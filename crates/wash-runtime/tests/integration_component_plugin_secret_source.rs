//! Integration tests for the `wasmcloud:host/secret-source` export: a
//! plugin-backed `secrets:` entry resolved through a real host component
//! plugin (`secret-source-plugin`) and delivered to another component plugin
//! (`kv-plugin`) as its binding config, read back through a caller workload.

#![cfg(feature = "host-component-plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use anyhow::Result;

use wash_runtime::host::HostApi;
use wash_runtime::plugin::{PluginBindingSet, PluginBindings, PluginSecretRef};
use wash_runtime::types::{LocalResources, WorkloadState};

mod common;
use common::{
    component_workload_request, kv_plugin_caller_host_interfaces, req,
    start_host_with_component_plugins,
};

const KV_PLUGIN_WASM: &[u8] = include_bytes!("wasm/kv_plugin.wasm");
const KV_PLUGIN_CALLER_WASM: &[u8] = include_bytes!("wasm/kv_plugin_caller.wasm");
const SECRET_SOURCE_WASM: &[u8] = include_bytes!("wasm/secret_source_plugin.wasm");
const KV_ID: &str = "acme-kv-plugin";
const SECRETS_ID: &str = "acme-secrets";

/// `acme-kv-plugin`'s entry reading `keys` (config key -> backend key) from
/// the `acme-secrets` plugin.
fn kv_reading_secrets(keys: &[(&str, &str)]) -> PluginBindings {
    let secrets = keys
        .iter()
        .map(|(config_key, key)| {
            (
                config_key.to_string(),
                PluginSecretRef {
                    source: "kv-secrets".into(),
                    plugin: SECRETS_ID.into(),
                    key: (*key).into(),
                },
            )
        })
        .collect();
    PluginBindings::new().with_plugin(PluginBindingSet::new(KV_ID).with_base_secrets(secrets))
}

async fn start(bindings: PluginBindings) -> Result<(std::net::SocketAddr, impl HostApi)> {
    start_host_with_component_plugins(
        "127.0.0.1:0",
        &[(KV_ID, KV_PLUGIN_WASM), (SECRETS_ID, SECRET_SOURCE_WASM)],
        bindings,
    )
    .await
}

fn caller(host: &str) -> wash_runtime::types::WorkloadStartRequest {
    component_workload_request(
        "kv-plugin-caller",
        host,
        KV_PLUGIN_CALLER_WASM,
        LocalResources::default(),
        kv_plugin_caller_host_interfaces(host),
    )
}

/// The secret reaches the consuming plugin's bind as config, and the source
/// was told which plugin and binding it was resolving for and which workload's
/// bind needed it.
#[tokio::test]
async fn a_plugin_secret_reaches_the_consuming_plugin() -> Result<()> {
    let host = "secret-source-ok";
    let (addr, h) = start(kv_reading_secrets(&[
        ("tier", "tier"),
        ("who", "who"),
        ("workload", "workload"),
    ]))
    .await?;
    let resp = h.workload_start(caller(host)).await?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Running,
        "{}",
        resp.workload_status.message
    );
    let client = reqwest::Client::new();
    let (status, body) = req(&client, &addr, host, "/bound-config?key=tier").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(body, "platinum");
    let (_, body) = req(&client, &addr, host, "/bound-config?key=who").await?;
    assert_eq!(body, format!("{KV_ID}/-"));
    let (_, body) = req(&client, &addr, host, "/bound-config?key=workload").await?;
    assert_eq!(body, format!("test/{host}"));
    Ok(())
}

/// A refusal fails the deploy with the source's case and message, a key the
/// source does not have fails it naming the key, and so does a binary value,
/// since plugin config holds text.
#[tokio::test]
async fn a_refused_or_missing_secret_fails_the_deploy() -> Result<()> {
    for (key, expect) in [
        ("denied", "access denied"),
        ("absent", "no key `absent`"),
        ("binary", "not UTF-8"),
    ] {
        let (_, h) = start(kv_reading_secrets(&[("tier", key)])).await?;
        let resp = h.workload_start(caller("secret-source-refused")).await?;
        assert_eq!(resp.workload_status.workload_state, WorkloadState::Error);
        assert!(
            resp.workload_status.message.contains(expect),
            "{key}: {}",
            resp.workload_status.message
        );
    }
    Ok(())
}

/// A secret naming the consuming plugin itself, or a plugin that is not
/// registered, fails the host at build rather than the first bind.
#[tokio::test]
async fn a_secret_that_could_never_resolve_fails_host_start() -> Result<()> {
    for (plugin, expect) in [(KV_ID, "its own config"), ("nope", "not registered")] {
        let bindings = PluginBindings::new().with_plugin(
            PluginBindingSet::new(KV_ID).with_base_secrets(HashMap::from([(
                "tier".to_string(),
                PluginSecretRef {
                    source: "kv-secrets".into(),
                    plugin: plugin.into(),
                    key: "tier".into(),
                },
            )])),
        );
        let Err(err) = start(bindings).await else {
            panic!("{plugin}: the host must refuse to start");
        };
        assert!(format!("{err:#}").contains(expect), "{plugin}: {err:#}");
    }
    Ok(())
}

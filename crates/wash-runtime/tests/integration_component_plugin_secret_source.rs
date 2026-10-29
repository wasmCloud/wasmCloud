//! Integration tests for the `wasmcloud:host/secret-source` export: a
//! plugin-backed `secrets:` entry resolved through a real host component
//! plugin (`secret-source-plugin`) and delivered to another component plugin
//! (`kv-plugin`) as its binding config, read back through a caller workload;
//! and the same source serving a workload its own secrets through the native
//! `wasmcloud-secrets` plugin, limited to the workloads the entry grants.

#![cfg(feature = "host-component-plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use anyhow::Result;

use wash_runtime::host::HostApi;
use wash_runtime::plugin::{
    PluginBindingSet, PluginBindings, PluginSecretRef, WorkloadGrant, WorkloadSelector,
};
use wash_runtime::types::{LocalResources, WorkloadState};

mod common;
use common::{
    component_workload_request, kv_plugin_caller_host_interfaces, req,
    secrets_caller_host_interfaces_with_config, secrets_interface,
    start_host_with_component_plugins,
};

const KV_PLUGIN_WASM: &[u8] = include_bytes!("wasm/kv_plugin.wasm");
const KV_PLUGIN_CALLER_WASM: &[u8] = include_bytes!("wasm/kv_plugin_caller.wasm");
const SECRET_SOURCE_WASM: &[u8] = include_bytes!("wasm/secret_source_plugin.wasm");
const SECRETS_CALLER_WASM: &[u8] = include_bytes!("wasm/secrets_caller.wasm");
/// Imports `wasmcloud:secrets/secret` under the label `db-password`.
const SECRETS_CONSUMER_WASM: &[u8] = include_bytes!("wasm/secrets_consumer_plugin.wasm");
/// The native plugin serving `wasmcloud:secrets` to workloads.
const WORKLOAD_SECRETS_ID: &str = "wasmcloud-secrets";
const KV_ID: &str = "acme-kv-plugin";
const SECRETS_ID: &str = "acme-secrets";

/// The namespace every test workload deploys into.
fn test_namespace() -> WorkloadGrant {
    WorkloadGrant::Only(vec![WorkloadSelector::namespace("test")])
}

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
                    grant: test_namespace(),
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
/// source does not have fails it naming the key, and so do a binary value,
/// since plugin config holds text, and a key answered twice.
#[tokio::test]
async fn a_refused_or_missing_secret_fails_the_deploy() -> Result<()> {
    for (key, expect) in [
        ("denied", "access denied"),
        ("absent", "no key `absent`"),
        ("binary", "not UTF-8"),
        ("twice", "answered `twice` more than once"),
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
                    grant: test_namespace(),
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

/// `wasmcloud-secrets`' entry serving workloads `db-password`, and
/// `requested-by` (the workload the source was told it resolved for), from
/// `acme-secrets` to the workloads `grant` admits.
fn workload_secrets(grant: WorkloadGrant) -> PluginBindings {
    let secret = |key: &str| PluginSecretRef {
        source: "billing-pw".into(),
        plugin: SECRETS_ID.into(),
        key: key.into(),
        grant: grant.clone(),
    };
    PluginBindings::new().with_plugin(
        PluginBindingSet::new(WORKLOAD_SECRETS_ID).with_base_secrets(HashMap::from([
            ("db-password".to_string(), secret("db-password")),
            ("requested-by".to_string(), secret("workload")),
        ])),
    )
}

/// A `secrets-caller` workload called `name` in namespace `test`, reading its
/// secrets through `wasmcloud:secrets/store` and `reveal`, with none of its
/// own config.
fn secrets_caller(name: &str) -> wash_runtime::types::WorkloadStartRequest {
    component_workload_request(
        "secrets-caller",
        name,
        SECRETS_CALLER_WASM,
        LocalResources::default(),
        secrets_caller_host_interfaces_with_config(name, HashMap::new()),
    )
}

/// A granted workload reads its own secret from the secret-source plugin with
/// `store.get` + `reveal`, and the source was told which workload asked.
#[tokio::test]
async fn a_granted_workload_reads_its_own_secret() -> Result<()> {
    for grant in [
        WorkloadSelector::workload("test", "billing-api"),
        WorkloadSelector::namespace("test"),
    ] {
        let (addr, h) = start(workload_secrets(WorkloadGrant::Only(vec![grant.clone()]))).await?;
        let resp = h.workload_start(secrets_caller("billing-api")).await?;
        assert_eq!(
            resp.workload_status.workload_state,
            WorkloadState::Running,
            "{grant:?}: {}",
            resp.workload_status.message
        );
        let client = reqwest::Client::new();
        let (status, body) = req(&client, &addr, "billing-api", "/get?key=db-password").await?;
        assert_eq!(status.as_u16(), 200, "{grant:?}");
        assert_eq!(body, "hunter2");
        let (_, body) = req(&client, &addr, "billing-api", "/get?key=requested-by").await?;
        assert_eq!(body, "test/billing-api");
    }
    Ok(())
}

/// A replica of a WorkloadDeployment has a generated name, so a grant naming
/// the deployment matches it through the operator's annotation, and the source
/// is told the deployment's name.
#[tokio::test]
async fn a_grant_by_name_matches_every_replica_of_a_deployment() -> Result<()> {
    let (addr, h) = start(workload_secrets(WorkloadGrant::Only(vec![
        WorkloadSelector::workload("test", "billing-api"),
    ])))
    .await?;
    let replica = "billing-api-5d8f9c-x2k4q";
    let mut request = secrets_caller(replica);
    request.workload.annotations.insert(
        wash_runtime::engine::workload::WORKLOAD_DEPLOYMENT_ANNOTATION.to_string(),
        "billing-api".to_string(),
    );
    let resp = h.workload_start(request).await?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Running,
        "{}",
        resp.workload_status.message
    );
    let client = reqwest::Client::new();
    let (_, body) = req(&client, &addr, replica, "/get?key=db-password").await?;
    assert_eq!(body, "hunter2");
    let (_, body) = req(&client, &addr, replica, "/get?key=requested-by").await?;
    assert_eq!(body, "test/billing-api");
    Ok(())
}

/// A workload outside the grant of an entry-level secret deploys without its
/// keys, so `store.get` finds nothing, while a granted one on the same host
/// reads it.
#[tokio::test]
async fn a_workload_outside_the_grant_cannot_read_the_secret() -> Result<()> {
    let (addr, h) = start(workload_secrets(WorkloadGrant::Only(vec![
        WorkloadSelector::workload("test", "billing-api"),
    ])))
    .await?;
    let client = reqwest::Client::new();
    for (name, expect) in [("intruder", None), ("billing-api", Some("hunter2"))] {
        let resp = h.workload_start(secrets_caller(name)).await?;
        assert_eq!(
            resp.workload_status.workload_state,
            WorkloadState::Running,
            "{name}: {}",
            resp.workload_status.message
        );
        let (status, body) = req(&client, &addr, name, "/get?key=db-password").await?;
        match expect {
            Some(value) => assert_eq!((status.as_u16(), body.as_str()), (200, value)),
            None => assert_eq!(status.as_u16(), 404, "{name}: {body}"),
        }
    }
    Ok(())
}

/// A workload whose labeled `db-password` import reads a secret withheld from
/// it fails to deploy, and the error says the key was withheld without naming
/// the entry or its grant.
#[tokio::test]
async fn a_labeled_import_of_a_withheld_secret_says_it_was_withheld() -> Result<()> {
    let (_, h) = start(workload_secrets(WorkloadGrant::Only(vec![
        WorkloadSelector::workload("test", "billing-api"),
    ])))
    .await?;
    for (name, granted) in [("intruder", false), ("billing-api", true)] {
        let mut labeled = secrets_interface(HashMap::new());
        labeled.interfaces = ["secret".to_string()].into_iter().collect();
        labeled.name = Some("db-password".to_string());
        let request = component_workload_request(
            "labeled-secret",
            name,
            SECRETS_CONSUMER_WASM,
            LocalResources::default(),
            vec![secrets_interface(HashMap::new()), labeled],
        );
        let resp = h.workload_start(request).await;
        let msg = match &resp {
            Ok(resp) => resp.workload_status.message.clone(),
            Err(e) => format!("{e:#}"),
        };
        let running = resp
            .as_ref()
            .is_ok_and(|r| r.workload_status.workload_state == WorkloadState::Running);
        assert_eq!(running, granted, "{name}: {msg}");
        if !granted {
            assert!(
                msg.contains(
                    "withheld from this workload, outside its secret's grant: `db-password`"
                ),
                "{msg}"
            );
            assert!(!msg.contains("billing-pw"), "{msg}");
            assert!(!msg.contains("billing-api"), "{msg}");
        }
    }
    Ok(())
}

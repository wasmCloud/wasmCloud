//! Integration test: a host component plugin (`secrets-consumer-plugin`)
//! importing a *native* host builtin (`wasmcloud:secrets`) instead of only
//! self-imports or WASI.
//!
//! `secrets-consumer-plugin` exports a bespoke `acme:secretsproxy/store`
//! capability whose `get-secret` resolves through the plugin's own imported
//! `wasmcloud:secrets/store` + `reveal` — which the host links against the
//! *native* `wasmcloud-secrets` plugin at plugin-load time, never against
//! another component plugin. The plugin's own bind-time config (its own
//! `secretFrom`-equivalent, delivered via `on-workload-bind` keyed by the
//! plugin's own id, never written to a file it reads) is what the native
//! plugin serves back through that import.
//!
//! Driven end to end over HTTP through the `secrets-consumer-plugin-caller`
//! workload, which imports `acme:secretsproxy/store` directly.
//!
//! Also covers a labeled `wasmcloud:secrets/secret` import (`db-password`),
//! which needs `wasm_component_model_implements` to parse the fixture's
//! `(implements ..)` annotation at all — matching every other
//! `(implements ..)` integration test's feature gate.

#![cfg(all(
    feature = "host-component-plugins",
    feature = "wasm_component_model_implements"
))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::component_host::ComponentHostPlugin;
use wash_runtime::plugin::wasmcloud_secrets::WasmcloudSecrets;
use wash_runtime::types::LocalResources;
use wash_runtime::wit::WitInterface;

mod common;
use common::{component_workload_request, http_incoming_handler_interface};

const CONSUMER_PLUGIN_WASM: &[u8] = include_bytes!("wasm/secrets_consumer_plugin.wasm");
const CALLER_WASM: &[u8] = include_bytes!("wasm/secrets_consumer_plugin_caller.wasm");
const PLUGIN_ID: &str = "secrets-consumer-plugin";

fn secretsproxy_interface() -> WitInterface {
    WitInterface {
        namespace: "acme".to_string(),
        package: "secretsproxy".to_string(),
        interfaces: ["store".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

async fn req(
    client: &reqwest::Client,
    addr: &std::net::SocketAddr,
    path: &str,
) -> Result<(reqwest::StatusCode, String)> {
    let resp = timeout(
        Duration::from_secs(15),
        client.get(format!("http://{addr}{path}")).send(),
    )
    .await
    .context("request timed out")??;
    let status = resp.status();
    let body = resp.text().await?;
    Ok((status, body))
}

/// A host component plugin importing `wasmcloud:secrets` resolves that import
/// against the native `wasmcloud-secrets` plugin, and the imported plugin's
/// own bind-time config is what the native serves back — proving both B1
/// (native-import linking) and B2 (bind delivery to a loading plugin) end to
/// end.
#[tokio::test]
async fn test_host_component_plugin_imports_native_secrets() -> Result<()> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();

    let builder = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(WasmcloudSecrets::new()))?;

    // The plugin's own resolved bind-time config — in production this comes
    // from the plugin's own `config`/`configFrom`/`secretFrom` manifest entry
    // (Phase E/F); here it's supplied directly, as `load_component_plugin`'s
    // real callers eventually will via `ComponentPluginSpec.config`.
    //
    // `api-key` backs the plugin's dynamic `store.get` import. `db-password`
    // backs its labeled `wasmcloud:secrets/secret` import, validated
    // structurally at `ComponentHostPlugin::new`, before the plugin is even
    // instantiated.
    let plugin_config = HashMap::from([
        ("api-key".to_string(), "s3cr3t-value".to_string()),
        ("db-password".to_string(), "hunter2".to_string()),
    ]);
    let native_plugins = builder.native_plugins();
    let http_handler = builder.http_handler();
    let plugin = ComponentHostPlugin::new(
        PLUGIN_ID,
        CONSUMER_PLUGIN_WASM,
        engine,
        &native_plugins,
        &plugin_config,
        Arc::from([]),
        Arc::from([]),
        http_handler,
    )
    .await
    .context("secrets-consumer-plugin should link against the native secrets plugin")?;

    let host = builder.with_plugin(Arc::new(plugin))?.build()?;
    let host = host.start().await.context("failed to start host")?;

    host.workload_start(component_workload_request(
        "secrets-consumer-plugin-caller",
        "caller",
        CALLER_WASM,
        LocalResources::default(),
        vec![
            http_incoming_handler_interface("caller", None),
            secretsproxy_interface(),
        ],
    ))
    .await?;

    let client = reqwest::Client::new();
    let (status, body) = req(&client, &addr, "/get?key=api-key").await?;
    assert_eq!(
        status.as_u16(),
        200,
        "the plugin's own bound secret must resolve through its native import"
    );
    assert_eq!(body, "s3cr3t-value");

    let (status, _) = req(&client, &addr, "/get?key=absent").await?;
    assert_eq!(status.as_u16(), 404, "an unbound key reads as none");

    let (status, body) = req(&client, &addr, "/get-db-password").await?;
    assert_eq!(
        status.as_u16(),
        200,
        "the plugin's labeled `db-password` secret must resolve through its \
         named `wasmcloud:secrets/secret` import"
    );
    assert_eq!(body, "hunter2");

    Ok(())
}

/// `secrets-consumer-plugin` imports `wasmcloud:secrets/secret` under the
/// label `db-password` — a labeled import needs no declaration or call to be
/// checked: the label itself, present in the plugin's own component type, is
/// what the host validates against its resolved config. A host started with
/// that key missing fails at `ComponentHostPlugin::new` — plugin
/// *construction*, before the plugin is even instantiated, let alone started
/// — naming the missing label.
#[tokio::test]
async fn test_missing_labeled_secret_fails_plugin_construction() -> Result<()> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;

    let builder = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(WasmcloudSecrets::new()))?;

    // `api-key` is present (so only the labeled-secret check is exercised);
    // `db-password` is missing.
    let plugin_config = HashMap::from([("api-key".to_string(), "s3cr3t-value".to_string())]);
    let native_plugins = builder.native_plugins();
    let http_handler = builder.http_handler();
    let result = ComponentHostPlugin::new(
        PLUGIN_ID,
        CONSUMER_PLUGIN_WASM,
        engine,
        &native_plugins,
        &plugin_config,
        Arc::from([]),
        Arc::from([]),
        http_handler,
    )
    .await;
    let Err(err) = result else {
        panic!("plugin construction must fail when a labeled secret's config is missing");
    };

    let msg = format!("{err:#}");
    assert!(
        msg.contains("db-password"),
        "expected the error to name the specific missing label, got: {msg}"
    );

    Ok(())
}

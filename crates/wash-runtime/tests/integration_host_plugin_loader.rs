//! Tests for [`wash_runtime::plugin::component_host::load_component_plugin`] —
//! the production loader that resolves a [`ComponentPluginSpec`] (from a
//! `wash host --host-plugin` flag or a `wash dev` config entry) to a
//! ready-to-register plugin. The file source is exercised unconditionally; the
//! OCI source runs opt-in against a live registry via `HOST_PLUGIN_OCI_REF`.
#![cfg(feature = "host-component-plugins")]

use std::path::PathBuf;

use anyhow::Result;
use wash_runtime::engine::Engine;
use wash_runtime::oci::{OciConfig, OciPullPolicy};
use wash_runtime::plugin::component_host::load_component_plugin;
use wash_runtime::plugin::{ComponentPluginSpec, HostPlugin as _, PluginSource};

fn kv_plugin_path() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/wasm/kv_plugin.wasm"
    ))
}

#[tokio::test]
async fn loads_component_plugin_from_file() -> Result<()> {
    let engine = Engine::builder().build()?;
    let spec = ComponentPluginSpec::from_plugin_source(
        "acme-kv-plugin",
        PluginSource::File(kv_plugin_path()),
    );

    let plugin = load_component_plugin(&spec, &engine, OciConfig::default()).await?;

    assert_eq!(plugin.id(), "acme-kv-plugin");
    let world = plugin.world();
    assert!(
        world
            .imports
            .iter()
            .any(|i| i.namespace == "acme" && i.package == "kv"),
        "expected the plugin's derived world to advertise acme:kv, got {world:?}"
    );
    Ok(())
}

/// Opt-in OCI-source loading against a live registry. Set `HOST_PLUGIN_OCI_REF`
/// to a pushed plugin (skips when unset, so CI is unaffected). To run locally:
///
/// ```text
/// docker run -d -p 5001:5000 registry:2
/// wash oci push --insecure localhost:5001/kv-plugin:test \
///     crates/wash-runtime/tests/wasm/kv_plugin.wasm
/// HOST_PLUGIN_OCI_REF=localhost:5001/kv-plugin:test \
///     cargo test -p wash-runtime --features host-component-plugins \
///     --test integration_host_plugin_loader loads_component_plugin_from_oci
/// ```
#[tokio::test]
async fn loads_component_plugin_from_oci() -> Result<()> {
    let Ok(reference) = std::env::var("HOST_PLUGIN_OCI_REF") else {
        eprintln!("HOST_PLUGIN_OCI_REF unset; skipping OCI loader test");
        return Ok(());
    };

    let engine = Engine::builder().build()?;
    let spec = ComponentPluginSpec::from_plugin_source(
        "acme-kv-plugin",
        PluginSource::Oci {
            image: reference.clone(),
            pull_policy: OciPullPolicy::Always,
        },
    );
    // Local registries speak plain HTTP.
    let oci_config = OciConfig {
        insecure: true,
        ..OciConfig::default()
    };

    let plugin = load_component_plugin(&spec, &engine, oci_config).await?;

    assert_eq!(plugin.id(), "acme-kv-plugin");
    assert!(
        plugin
            .world()
            .imports
            .iter()
            .any(|i| i.namespace == "acme" && i.package == "kv"),
        "expected acme:kv in the OCI-pulled plugin's world"
    );
    Ok(())
}

#[tokio::test]
async fn rejects_digest_pin_on_file_source() {
    let engine = Engine::builder().build().unwrap();
    let mut spec = ComponentPluginSpec::from_plugin_source(
        "acme-kv-plugin",
        PluginSource::File(kv_plugin_path()),
    );
    spec.expected_digest = Some("sha256:deadbeef".into());

    let err = load_component_plugin(&spec, &engine, OciConfig::default())
        .await
        .err()
        .expect("digest pin on a file source should fail to load");
    assert!(
        format!("{err:#}").contains("digest pinning"),
        "expected a digest-pinning error, got: {err:#}"
    );
}

#[tokio::test]
async fn missing_file_errors_with_id_and_context() {
    let engine = Engine::builder().build().unwrap();
    let spec = ComponentPluginSpec::from_plugin_source(
        "ghost",
        PluginSource::File("/nonexistent/does-not-exist.wasm".into()),
    );

    let err = load_component_plugin(&spec, &engine, OciConfig::default())
        .await
        .err()
        .expect("a missing file should fail to load");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("ghost") && msg.contains("failed to read"),
        "expected the error to name the plugin and the read failure, got: {msg}"
    );
}

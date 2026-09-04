//! Integration test: a workload importing a **host component plugin's**
//! interface under an `(implements ..)` label.
//!
//! The existing `(implements ..)` suites all route labels to *native* plugins
//! (`wasi:keyvalue`, `wasmcloud:postgres`, `wasmcloud:blobstore`,
//! `wasmcloud:nats`), and `integration_plugin_imports_native` covers a
//! component plugin consuming a native's labeled interface. Neither exercises
//! the direction this covers: a workload's labeled import resolved *by* a
//! component plugin.
//!
//! Three things have to line up for that, and each is asserted here:
//!
//!   1. The component model names a labeled import by its **label**, so the
//!      plugin must define the linker instance under the label rather than
//!      under the interface's own name. A workload that instantiates at all is
//!      the proof — a definition under the wrong name leaves the import
//!      unresolved and `workload_start` fails.
//!   2. The plain and labeled imports coexist on one linker, so declaring a
//!      binding must not cost the workload its unlabeled import.
//!   3. The label reaches the *capability call*, not only `on-workload-bind`:
//!      `whichbinding` returns what the plugin read from
//!      `wasmcloud:host/identity#get-binding-name`.
//!
//! The operator's `host.plugins` declaration is what turns label routing on
//! (`supports_named_instances`), so the host here declares `tenant-a` and
//! `tenant-b` the way an operator's Helm values would.

#![cfg(all(
    feature = "host-component-plugins",
    feature = "wasm_component_model_implements"
))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use anyhow::Result;

use wash_runtime::host::HostApi;
use wash_runtime::plugin::{PluginBindingSet, PluginBindings};
use wash_runtime::types::{LocalResources, WorkloadState};

mod common;
use common::{
    acme_kv_interface, component_workload_request, http_incoming_handler_interface,
    kv_plugin_implements_caller_host_interfaces, req, start_host_with_component_plugin_bindings,
};

const KV_PLUGIN_WASM: &[u8] = include_bytes!("wasm/kv_plugin.wasm");
const CALLER_WASM: &[u8] = include_bytes!("wasm/kv_plugin_implements_caller.wasm");
const PLUGIN_ID: &str = "acme-kv-plugin";

/// The operator's declaration: this plugin serves two named bindings. Nothing
/// is configured under them — what a binding reads is the plugin's business,
/// and an entry that configures nothing is the shape a declaration-only binding
/// takes.
fn declared_bindings() -> PluginBindings {
    PluginBindings::new().with_plugin(
        PluginBindingSet::new(PLUGIN_ID)
            .with_binding("tenant-a", HashMap::new())
            .with_binding("tenant-b", HashMap::new()),
    )
}

fn caller_workload(host: &str) -> wash_runtime::types::WorkloadStartRequest {
    component_workload_request(
        "kv-plugin-implements-caller",
        host,
        CALLER_WASM,
        LocalResources::default(),
        kv_plugin_implements_caller_host_interfaces(host),
    )
}

/// Every label the component imports is wired, and each call carries the label
/// it arrived on. The plain import carries none — that is a real answer, not a
/// degraded one, which is why the WIT returns `option<string>`.
#[tokio::test]
async fn a_labeled_import_reaches_the_plugin_under_its_label() -> Result<()> {
    let host = "kv-implements";
    let (addr, h) = start_host_with_component_plugin_bindings(
        "127.0.0.1:0",
        PLUGIN_ID,
        KV_PLUGIN_WASM,
        declared_bindings(),
    )
    .await?;
    // Instantiating at all proves the labeled instances were defined under
    // their labels: the component imports `tenant-a`/`tenant-b`, and a
    // definition under `acme:kv/store` would leave both unresolved.
    h.workload_start(caller_workload(host)).await?;
    let client = reqwest::Client::new();

    for (via, expected) in [
        ("plain", "<none>"),
        ("tenant-a", "tenant-a"),
        ("tenant-b", "tenant-b"),
    ] {
        let (status, body) = req(&client, &addr, host, &format!("/whichbinding?via={via}")).await?;
        assert_eq!(
            status.as_u16(),
            200,
            "/whichbinding via {via} should be 200"
        );
        assert_eq!(
            body, expected,
            "a call through the {via} import must report {expected} as its binding"
        );
    }
    Ok(())
}

/// The labels name bindings of one plugin, not separate backends: a value
/// written through one is readable through the others, because all three
/// imports route to the same store. What the label changes is the binding a
/// call carries, and nothing else.
#[tokio::test]
async fn labeled_imports_share_the_plugins_one_store() -> Result<()> {
    let host = "kv-implements-store";
    let (addr, h) = start_host_with_component_plugin_bindings(
        "127.0.0.1:0",
        PLUGIN_ID,
        KV_PLUGIN_WASM,
        declared_bindings(),
    )
    .await?;
    h.workload_start(caller_workload(host)).await?;
    let client = reqwest::Client::new();

    let (status, _) = req(
        &client,
        &addr,
        host,
        "/set?via=tenant-a&key=shared&value=written-via-a",
    )
    .await?;
    assert!(status.is_success(), "/set via tenant-a should succeed");

    for via in ["plain", "tenant-a", "tenant-b"] {
        let (status, body) =
            req(&client, &addr, host, &format!("/get?via={via}&key=shared")).await?;
        assert_eq!(status.as_u16(), 200, "/get via {via} should find the key");
        assert_eq!(
            body, "written-via-a",
            "every binding of one plugin reads the same store"
        );
    }
    Ok(())
}

/// A label nobody declared is refused, and the refusal names it.
///
/// This is the boundary that makes the operator's `bindings` block a grant
/// rather than a convention: a workload cannot reach a binding by naming one.
/// The plugin declares `tenant-a` only, and the workload's `tenant-b` entry
/// stops the deploy.
#[tokio::test]
async fn an_undeclared_label_is_refused() -> Result<()> {
    let host = "kv-implements-undeclared";
    let only_a = PluginBindings::new()
        .with_plugin(PluginBindingSet::new(PLUGIN_ID).with_binding("tenant-a", HashMap::new()));
    let (_addr, h) =
        start_host_with_component_plugin_bindings("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM, only_a)
            .await?;

    // A refused bind is reported on the status rather than as an `Err`: the
    // request was accepted and the workload failed, which is what an operator
    // reading `workload_state` sees.
    let status = h
        .workload_start(caller_workload(host))
        .await?
        .workload_status;
    assert_eq!(
        status.workload_state,
        WorkloadState::Error,
        "a workload naming an undeclared binding must not deploy"
    );
    let msg = status.message;
    assert!(
        msg.contains("tenant-b") && msg.contains(PLUGIN_ID),
        "the refusal must name the label and the plugin whose declaration to fix, got: {msg}"
    );
    Ok(())
}

/// Declaring nothing changes nothing: with no `host.plugins` entry this plugin
/// does not claim labels at all, so a workload declaring three bindings for one
/// package hits the same refusal any single-instance plugin gives. The
/// operator's declaration is what turns label routing on, and its absence is
/// not a silent downgrade.
#[tokio::test]
async fn a_plugin_with_no_declaration_does_not_claim_labels() -> Result<()> {
    let host = "kv-implements-undeclared-plugin";
    let (_addr, h) = start_host_with_component_plugin_bindings(
        "127.0.0.1:0",
        PLUGIN_ID,
        KV_PLUGIN_WASM,
        PluginBindings::new(),
    )
    .await?;

    let status = h
        .workload_start(caller_workload(host))
        .await?
        .workload_status;
    assert_eq!(
        status.workload_state,
        WorkloadState::Error,
        "an undeclared plugin must not serve named bindings"
    );
    let msg = status.message;
    assert!(
        msg.contains("does not support named instances") && msg.contains("acme:kv"),
        "the refusal must say the plugin serves one instance, got: {msg}"
    );
    Ok(())
}

/// A label the component imports but the *manifest* never named is refused at
/// bind, and the refusal names the label.
///
/// `WitWorld::uses` matches an entry to an import by package, ignoring labels,
/// so a manifest carrying only the plain entry still matches a component that
/// imports under labels. Wiring those labels anyway would let a component
/// reach the plugin under a name the workload never declared — and the
/// operator's `bindings` block is checked against declared entries, so that
/// would sidestep it. The import shape has to match its entry.
#[tokio::test]
async fn a_label_the_manifest_never_named_is_refused() -> Result<()> {
    let host = "kv-implements-plain-entry";
    let (_addr, h) = start_host_with_component_plugin_bindings(
        "127.0.0.1:0",
        PLUGIN_ID,
        KV_PLUGIN_WASM,
        declared_bindings(),
    )
    .await?;

    // Only the plain entry: the operator declared both labels, but this
    // workload's manifest asks for neither.
    let plain_only = vec![
        http_incoming_handler_interface(host, None),
        acme_kv_interface(),
    ];
    let status = h
        .workload_start(component_workload_request(
            "kv-plugin-implements-caller",
            host,
            CALLER_WASM,
            LocalResources::default(),
            plain_only,
        ))
        .await?
        .workload_status;
    assert_eq!(
        status.workload_state,
        WorkloadState::Error,
        "a component importing under a label its manifest never named must not deploy"
    );
    let msg = status.message;
    assert!(
        msg.contains("tenant-a") && msg.contains("name: tenant-a") && msg.contains(PLUGIN_ID),
        "the refusal must name the label, the entry to add, and the plugin, got: {msg}"
    );
    Ok(())
}

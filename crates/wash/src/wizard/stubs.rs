//! Rendering the files a generated project is made of: full wiring, `TODO` bodies.
//! Emitted code must survive the lints the generated workspace denies, and an HTTP
//! trigger always goes through wstd's `#[http_server]` (raw wit-bindgen fails to link).

use super::spec::{Capability, Edition, PlannedNode, Spec, Trigger, rust_ident};

/// Dependency versions the generated crates pin.
pub(super) const WIT_BINDGEN: &str = "0.60.0";
const WSTD: &str = "0.6";
const TONIC: &str = "0.14";
const PROST: &str = "0.14";

/// The generated project-local interface: one fallible string-in/string-out function.
fn interface_wit(name: &str, edition: Edition) -> String {
    let asyncness = if edition.is_p3() { "async " } else { "" };
    format!(
        "/// Generated link. Replace `invoke` with the real contract between\n\
         /// these two components.\n\
         interface {name} {{\n    \
         invoke: {asyncness}func(input: string) -> result<string, string>;\n\
         }}\n"
    )
}

// ---------------------------------------------------------------------------
// Capability table
// ---------------------------------------------------------------------------

/// Vendor an unpublished WIT package and point `wit fetch` at it; merges the
/// `[overrides]` entry into `wkg.toml` so multiple vendored packages coexist.
pub async fn vendor_wit(
    root: &std::path::Path,
    dir: &str,
    package_key: &str,
    files: &[(&str, &str)],
) -> anyhow::Result<()> {
    use anyhow::Context as _;

    let vendor = root.join("wit-vendor").join(dir);
    tokio::fs::create_dir_all(&vendor)
        .await
        .with_context(|| format!("failed to create {}", vendor.display()))?;
    for (name, text) in files {
        tokio::fs::write(vendor.join(name), text)
            .await
            .with_context(|| format!("failed to write {}", vendor.join(name).display()))?;
    }

    let wkg = root.join("wkg.toml");
    let mut existing = tokio::fs::read_to_string(&wkg).await.unwrap_or_default();
    let entry = format!("\"{package_key}\" = {{ path = \"wit-vendor/{dir}\" }}\n");
    if existing.contains(&entry) {
        return Ok(());
    }
    if existing.is_empty() {
        existing = String::from(
            "# These packages are served by the host but not yet published to a\n\
             # registry, so the project vendors their WIT. Delete an override (and\n\
             # its wit-vendor/ directory) once its package is published.\n\
             [overrides]\n",
        );
    }
    existing.push_str(&entry);
    tokio::fs::write(&wkg, existing)
        .await
        .with_context(|| format!("failed to write {}", wkg.display()))?;
    Ok(())
}

/// The `wasmcloud:messaging@0.3.0` WIT, vendored because the package is not yet
/// published to a registry; embedded from the runtime's own dependency copy.
pub fn messaging_wit_0_3() -> &'static str {
    include_str!("../../../wash-runtime/wit/deps/wasmcloud-messaging-0.3.0/package.wit")
}

/// The lint and release-profile policy every generated workspace carries.
pub(super) const WORKSPACE_POLICY: &str = "\
[workspace.lints.rust]
unsafe_code = \"deny\"

[workspace.lints.clippy]
unwrap_used = \"deny\"
expect_used = \"deny\"
panic = \"deny\"
indexing_slicing = \"deny\"

[profile.release]
codegen-units = 1
opt-level = \"s\"
strip = true
";

/// The `export!` epilogue every generated component ends with.
/// `export!` emits unsafe FFI shims, so the `allow(unsafe_code)` is scoped to one module.
pub(super) fn export_epilogue() -> &'static str {
    "\n#[allow(unsafe_code)]\nmod export {\n    \
     use super::{bindings, Component};\n    \
     bindings::export!(Component with_types_in bindings);\n}\n"
}

/// The p3 streamed-response tail: return the `Response` now, feed its body from a task.
/// The trailers future must seed `Ok(None)` — a bare `None` infers `Option<_>` and fails.
pub(super) fn streamed_response(payload: &str) -> String {
    format!(
        "        // The body is streamed: hand back the response now and write into it\n        \
         // from a spawned task.\n        \
         let (mut body_tx, body_rx) = bindings::wit_stream::new();\n        \
         // `Response::new` takes the trailers future as\n        \
         // `Result<Option<Fields>, ErrorCode>`, so the default has to be\n        \
         // `Ok(None)` — a bare `None` infers `Option<_>` and does not compile.\n        \
         let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));\n\n        \
         wit_bindgen::spawn_local(async move {{\n            \
         body_tx.write_all({payload}).await;\n            \
         drop(body_tx);\n            \
         let _ = trailers_tx.write(Ok(None)).await;\n        \
         }});\n\n        \
         let (response, _result) = Response::new(Fields::new(), Some(body_rx), trailers_rx);\n        \
         Ok(response)\n"
    )
}

/// Name of the generated helper that exercises a capability, when it has one.
fn capability_fn(capability: &Capability) -> Option<&'static str> {
    Some(match capability {
        Capability::Keyvalue => "touch_keyvalue",
        Capability::Blobstore => "touch_blobstore",
        Capability::Config => "read_config",
        Capability::Logging => "write_log",
        Capability::Postgres => "query_database",
        Capability::Secrets => "read_secret",
        Capability::Webgpu => "touch_gpu",
        Capability::HttpEgress { .. } => "fetch_upstream",
        Capability::Grpc { .. } => "call_grpc",
        Capability::Otel => return None,
    })
}

/// The helper body for one capability. Wiring only; the work is a `TODO`.
pub fn capability_helper(capability: &Capability, edition: Edition) -> String {
    match (capability, edition) {
        (Capability::Keyvalue, Edition::P2) => keyvalue_helper(),
        (Capability::Keyvalue, Edition::P3) => keyvalue_helper_p3(),
        (Capability::Blobstore, Edition::P2) => blobstore_helper(),
        (Capability::Blobstore, Edition::P3) => blobstore_helper_p3(),
        (Capability::Config, _) => config_helper(),
        (Capability::Logging, _) => logging_helper(),
        (Capability::Postgres, Edition::P2) => postgres_helper(),
        (Capability::Postgres, Edition::P3) => postgres_helper_p3(),
        // p3-only by validation; the arms exist for exhaustiveness.
        (Capability::Secrets, _) => secrets_helper(),
        (Capability::Webgpu, _) => webgpu_helper(),
        (Capability::HttpEgress { host }, Edition::P2) => egress_helper(host),
        (Capability::HttpEgress { host }, Edition::P3) => egress_helper_p3(host),
        (Capability::Grpc { host }, _) => grpc_helper(host),
        (Capability::Otel, _) => String::new(),
    }
}

/// Async keyvalue against `wasmcloud:keyvalue/store@0.2.0`.
fn keyvalue_helper_p3() -> String {
    String::from(
        "\n/// TODO: replace with real storage. Opens the default bucket and\n\
         /// round-trips one key; `wash dev` serves this from memory with no\n\
         /// configuration.\n\
         async fn touch_keyvalue() -> String {\n    \
         use bindings::wasmcloud::keyvalue::store;\n\n    \
         let Ok(bucket) = store::open(\"default\".to_string()).await else {\n        \
         return String::from(\"keyvalue: unavailable\\n\");\n    \
         };\n    \
         if bucket.set(\"wizard-key\".to_string(), b\"wizard-value\".to_vec(), None).await.is_err() {\n        \
         return String::from(\"keyvalue: write failed\\n\");\n    \
         }\n    \
         match bucket.get(\"wizard-key\".to_string()).await {\n        \
         Ok(Some(bytes)) => format!(\"keyvalue: {}\\n\", String::from_utf8_lossy(&bytes)),\n        \
         Ok(None) => String::from(\"keyvalue: missing\\n\"),\n        \
         Err(_) => String::from(\"keyvalue: read failed\\n\"),\n    \
         }\n\
         }\n",
    )
}

/// Async blobstore against `wasmcloud:blobstore@0.1.0`, round-tripping one
/// object through native streams (fed from a spawned task to avoid deadlock).
fn blobstore_helper_p3() -> String {
    String::from(
        "\n/// TODO: replace with real object storage. Round-trips one object\n\
         /// through native streams; `wash dev` serves this from memory.\n\
         async fn touch_blobstore() -> String {\n    \
         use bindings::wasmcloud::blobstore::blobstore;\n\n    \
         let container = match blobstore::create_container(\"wizard\".to_string()).await {\n        \
         Ok(container) => container,\n        \
         Err(_) => match blobstore::get_container(\"wizard\".to_string()).await {\n            \
         Ok(container) => container,\n            \
         Err(err) => return format!(\"blobstore unavailable: {err}\\n\"),\n        \
         },\n    \
         };\n\n    \
         // Feed the body from a spawned task: `write_data` resolves only once\n    \
         // the host has fully drained the stream, so writing inline first\n    \
         // would deadlock.\n    \
         let (mut tx, rx) = bindings::wit_stream::new();\n    \
         wit_bindgen::spawn_local(async move {\n        \
         tx.write_all(b\"wizard-object\".to_vec()).await;\n        \
         drop(tx);\n    \
         });\n    \
         if container.write_data(\"wizard-key\".to_string(), rx).await.is_err() {\n        \
         return String::from(\"blobstore: write failed\\n\");\n    \
         }\n    \
         match container.get_data(\"wizard-key\".to_string(), 0, u64::MAX).await {\n        \
         Ok(stream) => format!(\n            \
         \"blobstore: {}\\n\",\n            \
         String::from_utf8_lossy(&stream.collect().await)\n        \
         ),\n        \
         Err(_) => String::from(\"blobstore: read failed\\n\"),\n    \
         }\n\
         }\n",
    )
}

/// Async postgres against `wasmcloud:postgres@0.2.0`; the result rows stream and are drained.
fn postgres_helper_p3() -> String {
    String::from(
        "\n/// TODO: replace with a real query. Needs `dev.postgres_url` in\n\
         /// `.wash/config.yaml` pointing at a database; without one the call\n\
         /// returns an error rather than panicking.\n\
         async fn query_database() -> String {\n    \
         use bindings::wasmcloud::postgres::query;\n\n    \
         match query::query(\"SELECT 1\".to_string(), vec![]).await {\n        \
         Ok((_columns, rows, _done)) => {\n            \
         // The rows arrive as a stream; drain it rather than hold it.\n            \
         let count = rows.collect().await.len();\n            \
         format!(\"postgres: {count} row(s)\\n\")\n        \
         }\n        \
         Err(err) => format!(\"postgres unavailable: {err:?}\\n\"),\n    \
         }\n\
         }\n",
    )
}

/// Secrets against `wasmcloud:secrets@2.1.0`; reports the value's length, never the value.
fn secrets_helper() -> String {
    String::from(
        "\n/// TODO: fetch the secrets this component actually needs. The key\n\
         /// below is seeded in `.wash/config.yaml` under `dev.host_interfaces`\n\
         /// — development values only; production delivers them at bind time.\n\
         async fn read_secret() -> String {\n    \
         use bindings::wasmcloud::secrets::store::SecretValue;\n    \
         use bindings::wasmcloud::secrets::{reveal, store};\n\n    \
         match store::get(\"api-key\".to_string()).await {\n        \
         Ok(secret) => {\n            \
         // Reveal at the last moment, and never log the value itself.\n            \
         match reveal::reveal(&secret).await {\n                \
         SecretValue::String(value) => {\n                    \
         format!(\"secret: api-key present ({} chars)\\n\", value.len())\n                \
         }\n                \
         SecretValue::Bytes(value) => {\n                    \
         format!(\"secret: api-key present ({} bytes)\\n\", value.len())\n                \
         }\n            \
         }\n        \
         }\n        \
         Err(err) => format!(\"secret unavailable: {err:?}\\n\"),\n    \
         }\n\
         }\n",
    )
}

/// GPU presence check against `wasi:webgpu`; requesting the adapter proves the wiring.
fn webgpu_helper() -> String {
    String::from(
        "\n/// TODO: replace with real GPU work — a shader module, buffers, a\n\
         /// pipeline. Requesting the adapter proves the capability is wired.\n\
         async fn touch_gpu() -> String {\n    \
         use bindings::wasi::webgpu::webgpu;\n\n    \
         let gpu = webgpu::get_gpu();\n    \
         match gpu.request_adapter(None).await {\n        \
         Some(_adapter) => String::from(\"webgpu: adapter ready\\n\"),\n        \
         None => String::from(\"webgpu: no adapter (headless host?)\\n\"),\n    \
         }\n\
         }\n",
    )
}

/// Outbound HTTP in p3: an import of the same unified `wasi:http/handler` the ingress exports.
fn egress_helper_p3(host: &str) -> String {
    format!(
        "\n/// TODO: call the endpoint this component actually needs. `{host}` is\n\
         /// allowlisted in `.wash/config.yaml`; the host denies anything that is not.\n\
         async fn fetch_upstream() -> String {{\n    \
         use bindings::wasi::http::handler;\n    \
         use bindings::wasi::http::types::{{Fields, Request, Scheme}};\n\n    \
         let (request, _) = Request::new(\n        \
         Fields::new(),\n        \
         None,\n        \
         bindings::wit_future::new(|| Ok(None)).1,\n        \
         None,\n    \
         );\n    \
         let _ = request.set_scheme(Some(&Scheme::Https));\n    \
         let _ = request.set_authority(Some(\"{host}\"));\n    \
         let _ = request.set_path_with_query(Some(\"/\"));\n\n    \
         match handler::handle(request).await {{\n        \
         Ok(response) => format!(\"upstream {host}: {{}}\\n\", response.get_status_code()),\n        \
         Err(err) => format!(\"upstream {host} failed: {{err}}\\n\"),\n    \
         }}\n\
         }}\n"
    )
}

fn keyvalue_helper() -> String {
    String::from(
        "\n/// TODO: replace with real storage. Opens the default bucket and\n\
         /// round-trips one key; `wash dev` serves this from memory with no\n\
         /// configuration, or point `dev.wasi_keyvalue_path` at a directory.\n\
         fn touch_keyvalue() -> String {\n    \
         use bindings::wasi::keyvalue::store;\n\n    \
         let Ok(bucket) = store::open(\"default\") else {\n        \
         return String::from(\"keyvalue: unavailable\\n\");\n    \
         };\n    \
         if bucket.set(\"wizard-key\", b\"wizard-value\").is_err() {\n        \
         return String::from(\"keyvalue: write failed\\n\");\n    \
         }\n    \
         match bucket.get(\"wizard-key\") {\n        \
         Ok(Some(bytes)) => format!(\"keyvalue: {}\\n\", String::from_utf8_lossy(&bytes)),\n        \
         Ok(None) => String::from(\"keyvalue: missing\\n\"),\n        \
         Err(_) => String::from(\"keyvalue: read failed\\n\"),\n    \
         }\n\
         }\n",
    )
}

fn blobstore_helper() -> String {
    String::from(
        "\n/// TODO: replace with real object storage. Creating the container is\n\
         /// enough to prove the capability is wired; writing an object needs an\n\
         /// `outgoing-value` stream, which belongs with the real logic.\n\
         fn touch_blobstore() -> String {\n    \
         use bindings::wasi::blobstore::blobstore;\n\n    \
         match blobstore::create_container(&String::from(\"wizard\")) {\n        \
         Ok(_container) => String::from(\"blobstore: container ready\\n\"),\n        \
         Err(err) => format!(\"blobstore unavailable: {err}\\n\"),\n    \
         }\n\
         }\n",
    )
}

fn config_helper() -> String {
    String::from(
        "\n/// TODO: read the keys this component actually needs. The one below is\n\
         /// seeded in `.wash/config.yaml` under `workload.config`; add more there\n\
         /// and they appear here without touching the host.\n\
         fn read_config() -> String {\n    \
         use bindings::wasi::config::store;\n\n    \
         // `get` is Ok(None) for an unset key and Err only on a store fault.\n    \
         match store::get(\"wizard.greeting\") {\n        \
         Ok(Some(value)) => format!(\"config: wizard.greeting={value}\\n\"),\n        \
         Ok(None) => String::from(\"config: wizard.greeting unset\\n\"),\n        \
         Err(err) => format!(\"config unavailable: {err:?}\\n\"),\n    \
         }\n\
         }\n",
    )
}

fn logging_helper() -> String {
    String::from(
        "\n/// TODO: log what matters. Output goes to the host's log, so it shows\n\
         /// up in `wash dev` alongside the runtime's own lines.\n\
         fn write_log() -> String {\n    \
         use bindings::wasi::logging::logging::{log, Level};\n\n    \
         log(Level::Info, \"wizard\", \"generated component reached its logging stub\");\n    \
         String::from(\"logging: wrote one line\\n\")\n\
         }\n",
    )
}

fn postgres_helper() -> String {
    String::from(
        "\n/// TODO: replace with a real query. Needs `dev.postgres_url` in\n\
         /// `.wash/config.yaml` pointing at a database; without one the call\n\
         /// returns an error rather than panicking.\n\
         fn query_database() -> String {\n    \
         use bindings::wasmcloud::postgres::query;\n\n    \
         match query::query(\"SELECT 1\", &[]) {\n        \
         Ok(rows) => format!(\"postgres: {} row(s)\\n\", rows.len()),\n        \
         Err(err) => format!(\"postgres unavailable: {err:?}\\n\"),\n    \
         }\n\
         }\n",
    )
}

/// Outbound HTTP; the generated config allowlists `host`, since the host denies the rest.
fn egress_helper(host: &str) -> String {
    format!(
        "\n/// TODO: call the endpoint this component actually needs. `{host}` is\n\
         /// allowlisted in `.wash/config.yaml`; the host denies anything that is not.\n\
         fn fetch_upstream() -> String {{\n    \
         use bindings::wasi::http::types::{{Headers, OutgoingRequest, Scheme}};\n\n    \
         let request = OutgoingRequest::new(Headers::new());\n    \
         let _ = request.set_scheme(Some(&Scheme::Https));\n    \
         let _ = request.set_authority(Some(\"{host}\"));\n    \
         let _ = request.set_path_with_query(Some(\"/\"));\n\n    \
         match bindings::wasi::http::outgoing_handler::handle(request, None) {{\n        \
         Ok(response) => {{\n            \
         response.subscribe().block();\n            \
         match response.get() {{\n                \
         Some(Ok(Ok(incoming))) => format!(\"upstream {host}: {{}}\\n\", incoming.status()),\n                \
         _ => format!(\"upstream {host}: no response\\n\"),\n            \
         }}\n        \
         }}\n        \
         Err(err) => format!(\"upstream {host} failed: {{err}}\\n\"),\n    \
         }}\n\
         }}\n"
    )
}

/// gRPC over `wasi:http/outgoing-handler`: ships the complete tonic-to-wstd
/// transport bridge, since that is the only genuinely hard part.
fn grpc_helper(host: &str) -> String {
    format!(
        r#"
/// Protobuf types generated from `proto/wizard.proto` by `build.rs`.
pub mod proto {{
    #![allow(clippy::all)]
    tonic::include_proto!("wizard");
}}

/// Bridges tonic/tower to the wstd HTTP client.
///
/// tonic normally talks to a hyper transport, which does not exist on wasm.
/// This carries each request out through `wasi:http/outgoing-handler` instead,
/// rewriting the URI onto `{host}` and converting the body in both directions.
struct WasiGrpcService {{
    base_uri: http::Uri,
}}

impl tower_service::Service<http::Request<tonic::body::Body>> for WasiGrpcService {{
    type Response =
        http::Response<http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, tonic::Status>>;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {{
        std::task::Poll::Ready(Ok(()))
    }}

    fn call(&mut self, req: http::Request<tonic::body::Body>) -> Self::Future {{
        use http_body_util::BodyExt as _;
        let base_uri = self.base_uri.clone();

        Box::pin(async move {{
            // tonic sets the method path; the authority comes from the base.
            let mut parts = base_uri.into_parts();
            if let Some(path_and_query) = req.uri().path_and_query().cloned() {{
                parts.path_and_query = Some(path_and_query);
            }}
            let uri = http::Uri::from_parts(parts)?;

            let (mut head, body) = req.into_parts();
            head.uri = uri;

            let collected = body
                .collect()
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {{ Box::new(e) }})?
                .to_bytes();
            let request = http::Request::from_parts(head, wstd::http::Body::from(collected));

            let response = wstd::http::Client::new()
                .send(request)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {{ e.into() }})?;

            let (head, body) = response.into_parts();
            let body = body
                .into_boxed_body()
                .map_err(|e| tonic::Status::internal(format!("{{e:#}}")));
            Ok(http::Response::from_parts(
                head,
                http_body_util::combinators::UnsyncBoxBody::new(body),
            ))
        }})
    }}
}}

/// TODO: call the method this component actually needs. The transport below is
/// complete — `{host}` is allowlisted in `.wash/config.yaml`, and swapping in
/// another generated client works the same way.
async fn call_grpc() -> String {{
    let service = WasiGrpcService {{
        base_uri: http::Uri::from_static("https://{host}"),
    }};
    let mut client = proto::wizard_client::WizardClient::new(service);

    let request = tonic::Request::new(proto::WizardRequest {{
        input: String::from("hello from wash wizard"),
    }});

    match client.invoke(request).await {{
        Ok(response) => format!("grpc {host}: {{}}\n", response.into_inner().output),
        Err(status) => format!("grpc {host} failed: {{}}\n", status.message()),
    }}
}}
"#
    )
}

/// The minimal `.proto` a gRPC component compiles: one service, one method.
pub fn grpc_proto() -> String {
    String::from(
        "syntax = \"proto3\";\n\n\
         package wizard;\n\n\
         // TODO: replace with the real service contract.\n\
         service Wizard {\n  \
         rpc Invoke(WizardRequest) returns (WizardResponse);\n\
         }\n\n\
         message WizardRequest {\n  \
         string input = 1;\n\
         }\n\n\
         message WizardResponse {\n  \
         string output = 1;\n\
         }\n",
    )
}

/// Build script compiling the `.proto` into Rust.
pub fn grpc_build_rs() -> String {
    String::from(
        "//! Compiles `proto/wizard.proto` into Rust types at build time.\n\n\
         fn main() -> Result<(), Box<dyn std::error::Error>> {\n    \
         println!(\"cargo:rerun-if-changed=proto/wizard.proto\");\n    \
         tonic_prost_build::configure()\n        \
         .build_client(true)\n        \
         .build_server(false)\n        \
         // wasm has no transport; generating it fails to compile.\n        \
         .build_transport(false)\n        \
         .compile_protos(&[\"proto/wizard.proto\"], &[\"proto\"])?;\n    \
         Ok(())\n\
         }\n",
    )
}

// ---------------------------------------------------------------------------
// WIT
// ---------------------------------------------------------------------------

/// The single `wit/world.wit` every component's bindings are generated from.
pub fn world_wit(spec: &Spec) -> String {
    let nodes = spec.plan();
    let mut out = format!("package {};\n\n", spec.wit_package());

    out.push_str(
        "// One interface per edge, never one shared by several components.\n\
         // The host resolves a component's imports against a map of every\n\
         // interface exported in the workload; an interface exported by two\n\
         // or more components is ambiguous, gets dropped from that map, and\n\
         // then no importer of it can link.\n\n",
    );

    for export in nodes.iter().filter_map(|n| n.exports.as_deref()) {
        out.push_str(&interface_wit(export, spec.edition));
        out.push('\n');
    }

    for node in &nodes {
        out.push_str(&format!("world {} {{\n", node.world));

        for import in &node.imports {
            out.push_str(&format!("    import {import};\n"));
        }
        // WIT rejects a world importing the same interface twice, so dedupe
        // across capabilities and the trigger's own lines.
        let mut seen = std::collections::BTreeSet::new();
        let mut push_import = |out: &mut String, import: &'static str| {
            if seen.insert(import) {
                out.push_str(&format!("    import {import};\n"));
            }
        };
        // Under p2 the wstd macro supplies the HTTP export; declaring it here
        // produces a component that fails to link against the messaging consumer.
        let p3_http_trigger =
            spec.edition.is_p3() && node.is_trigger && spec.trigger == Trigger::Http;
        if p3_http_trigger {
            push_import(&mut out, spec.edition.http_types_import());
        }
        // A p3 service sleeps on the monotonic clock between rounds of work.
        if spec.edition.is_p3() && node.is_trigger && spec.trigger == Trigger::Service {
            push_import(&mut out, spec.edition.monotonic_clock_import());
        }
        for capability in &node.capabilities {
            for import in capability.wit_imports(spec.edition) {
                push_import(&mut out, import);
            }
        }
        if p3_http_trigger {
            out.push_str(&format!(
                "    export {};\n",
                spec.edition.http_ingress_export()
            ));
        }
        // A messaging trigger only handles — it never publishes — so it does
        // not import the consumer.
        if spec.links_over_messaging() && node.is_trigger {
            push_import(&mut out, spec.edition.messaging_consumer());
        }
        if node.role == wash_topology::Role::Worker {
            out.push_str(&format!(
                "    export {};\n",
                spec.edition.messaging_handler()
            ));
            if !node.is_trigger {
                // A fan-out worker replies to whoever asked.
                push_import(&mut out, spec.edition.messaging_consumer());
            }
        }
        if spec.edition.is_p3() && node.is_trigger && spec.trigger == Trigger::Service {
            out.push_str(&format!("    export {};\n", spec.edition.cli_run_export()));
        }

        if let Some(export) = &node.exports {
            out.push_str(&format!("    export {export};\n"));
        }
        out.push_str("}\n\n");
    }
    out
}

// ---------------------------------------------------------------------------
// Cargo manifests
// ---------------------------------------------------------------------------

/// Workspace manifest, carrying the same lint denials the shipped templates use.
pub fn workspace_cargo_toml(spec: &Spec) -> String {
    let members = spec
        .plan()
        .iter()
        .map(|n| format!("\"{}\"", n.id))
        .collect::<Vec<_>>()
        .join(", ");

    let grpc_deps = if spec.uses_grpc() {
        format!(
            "tonic = {{ version = \"{TONIC}\", default-features = false, features = [\"codegen\"] }}\n\
             tonic-prost = \"{TONIC}\"\n\
             prost = \"{PROST}\"\n\
             tonic-prost-build = \"{TONIC}\"\n\
             tower-service = \"0.3\"\n\
             http = \"1\"\n\
             http-body-util = \"0.1\"\n\
             bytes = \"1\"\n"
        )
    } else {
        String::new()
    };

    format!(
        r#"[workspace]
resolver = "3"
members = [{members}]
package.edition = "2024"

[workspace.dependencies]
wit-bindgen = "{WIT_BINDGEN}"
wstd = "{WSTD}"
{grpc_deps}
{WORKSPACE_POLICY}"#
    )
}

/// Per-component manifest; every component is a cdylib reactor.
pub fn component_cargo_toml(spec: &Spec, node: &PlannedNode) -> String {
    let crate_type = "\n[lib]\ncrate-type = [\"cdylib\"]\n";

    // wstd supplies the p2 HTTP macro and `block_on`; p3 is uniformly async and needs neither.
    let needs_wstd = !spec.edition.is_p3()
        && ((node.is_trigger && spec.trigger == Trigger::Http)
            || node.capabilities.iter().any(|c| c.is_async(spec.edition)));

    let mut deps = String::new();
    // p3 exports are async, which wit-bindgen only generates with these on.
    if spec.edition.is_p3() {
        deps.push_str(
            "wit-bindgen = { workspace = true, features = [\"async-spawn\", \"inter-task-wakeup\"] }\n",
        );
    } else {
        deps.push_str("wit-bindgen = { workspace = true }\n");
    }
    if needs_wstd {
        deps.push_str("wstd = { workspace = true }\n");
    }

    let (grpc_deps, build_deps) = if node.capabilities.iter().any(is_grpc) {
        (
            "tonic = { workspace = true }\ntonic-prost = { workspace = true }\n\
             prost = { workspace = true }\ntower-service = { workspace = true }\n\
             http = { workspace = true }\nhttp-body-util = { workspace = true }\n\
             bytes = { workspace = true }\n",
            "\n[build-dependencies]\ntonic-prost-build = { workspace = true }\n",
        )
    } else {
        ("", "")
    };

    format!(
        r#"[package]
name = "{id}"
version = "0.1.0"
edition.workspace = true

[lints]
workspace = true
{crate_type}
[dependencies]
{deps}{grpc_deps}{build_deps}"#,
        id = node.id
    )
}

fn is_grpc(capability: &Capability) -> bool {
    matches!(capability, Capability::Grpc { .. })
}

impl Spec {
    /// Whether any component needs the protobuf toolchain.
    pub fn uses_grpc(&self) -> bool {
        self.capabilities.iter().any(is_grpc)
    }
}

// ---------------------------------------------------------------------------
// Component source
// ---------------------------------------------------------------------------

/// `src/lib.rs` — or `src/main.rs` for a service — for one component.
pub fn component_source(spec: &Spec, node: &PlannedNode) -> String {
    let header = "//! Generated by `wash wizard`. The wiring is complete; the bodies are\n\
                  //! not. Replace each `TODO` with the work this component should do.\n\n";

    if node.is_trigger && spec.trigger == Trigger::Service {
        let mut out = format!("{header}{}", bindings_block(spec, node));
        out.push_str(&use_lines(spec, node));
        out.push_str("struct Component;\n\n");
        out.push_str(&service_run_impl(spec, node));
        out.push_str(export_epilogue());
        return out;
    }
    if node.is_trigger && spec.trigger == Trigger::Http && spec.edition.is_p3() {
        return format!(
            "{header}{}{}",
            bindings_block(spec, node),
            p3_http_trigger(spec, node)
        );
    }
    // Under p2 it is a wstd `http_server`, which supplies its own export.
    if node.is_trigger && spec.trigger == Trigger::Http {
        return format!(
            "{header}{}{}",
            bindings_block(spec, node),
            http_trigger_main(spec, node)
        );
    }

    let mut out = format!("{header}{}", bindings_block(spec, node));
    out.push_str(&use_lines(spec, node));
    out.push_str("struct Component;\n\n");

    if node.role == wash_topology::Role::Worker {
        out.push_str(&worker_impl(spec, node));
    }
    if let Some(export) = &node.exports {
        out.push_str(&export_impl(spec, node, export));
    }

    out.push_str(export_epilogue());
    out
}

fn bindings_block(spec: &Spec, node: &PlannedNode) -> String {
    // wit-bindgen generates an async export only when told which functions are async.
    let asynchrony = if spec.edition.is_p3() && node.is_trigger && spec.trigger == Trigger::Http {
        format!(
            "        async: [\"export:{}#handle\"],\n",
            spec.edition.http_ingress_export()
        )
    } else {
        String::new()
    };
    format!(
        "mod bindings {{\n    \
         wit_bindgen::generate!({{\n        \
         // Every component in this workload shares the one `wit/` directory\n        \
         // at the workspace root.\n        \
         path: \"../wit\",\n        \
         world: \"{world}\",\n        \
         generate_all,\n\
         {asynchrony}    \
         }});\n}}\n\n",
        world = node.world
    )
}

/// A p3 HTTP trigger: an async `Guest` impl whose response body streams from a spawned task.
fn p3_http_trigger(spec: &Spec, node: &PlannedNode) -> String {
    let mut uses = use_lines(spec, node);
    if spec.links_over_messaging() {
        uses.push_str(
            "/// How long to wait for a worker to reply.\nconst TIMEOUT_MS: u32 = 5_000;\n\n",
        );
    }
    format!(
        "{uses}use bindings::exports::wasi::http::handler::Guest as Handler;\n\
         use bindings::wasi::http::types::{{ErrorCode, Fields, Request, Response}};\n\n\
         struct Component;\n\n\
         impl Handler for Component {{\n    \
         /// TODO: handle the request. Everything below is generated wiring.\n    \
         async fn handle(_request: Request) -> Result<Response, ErrorCode> {{\n        \
         #[allow(unused_mut)]\n        \
         let mut text = String::from(\"hello from wash wizard\\n\");\n\
         {calls}\n\
         {response}    \
         }}\n\
         }}\n{epilogue}",
        calls = downstream_calls(spec, node, "        ", true),
        response = streamed_response("text.into_bytes()"),
        epilogue = export_epilogue(),
    )
}

fn use_lines(spec: &Spec, node: &PlannedNode) -> String {
    let ns = spec.wit_namespace_ident();
    let mut out = String::new();
    for import in &node.imports {
        out.push_str(&format!(
            "use bindings::{ns}::app::{};\n",
            rust_ident(import)
        ));
    }
    if node.role == wash_topology::Role::Worker {
        if spec.edition.is_p3() {
            out.push_str(
                "use bindings::wasmcloud::messaging::types::{BrokerMessage, HandleMessageError};\n",
            );
        } else {
            out.push_str("use bindings::wasmcloud::messaging::types::BrokerMessage;\n");
        }
        if !node.is_trigger {
            out.push_str("use bindings::wasmcloud::messaging::consumer;\n");
        }
    }
    if spec.links_over_messaging() && node.is_trigger {
        out.push_str("use bindings::wasmcloud::messaging::consumer;\n");
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Calls into every downstream component and capability, appending to `text`.
fn downstream_calls(spec: &Spec, node: &PlannedNode, indent: &str, is_async: bool) -> String {
    let mut out = String::new();

    // Only the trigger publishes; `TIMEOUT_MS` exists only where the fan-out is issued.
    if spec.links_over_messaging() && node.is_trigger {
        for worker in 1..=spec.branches.len() {
            if spec.edition.is_p3() {
                // `request` resolves only once the callee has consumed the body
                // stream, so it is fed from a spawned task.
                out.push_str(&format!(
                    "{indent}let (mut tx{worker}, body{worker}) = bindings::wit_stream::new();\n\
                     {indent}wit_bindgen::spawn_local(async move {{\n\
                     {indent}    tx{worker}.write_all(b\"hello\".to_vec()).await;\n\
                     {indent}    drop(tx{worker});\n\
                     {indent}}});\n\
                     {indent}match consumer::request(\"tasks.worker{worker}\".to_string(), body{worker}, Some(TIMEOUT_MS)).await {{\n\
                     {indent}    Ok(reply) => text.push_str(&format!(\n\
                     {indent}        \"worker{worker}: {{}}\\n\",\n\
                     {indent}        String::from_utf8_lossy(&reply.body.collect().await)\n\
                     {indent}    )),\n\
                     {indent}    Err(err) => text.push_str(&format!(\"worker{worker} failed: {{err:?}}\\n\")),\n\
                     {indent}}}\n"
                ));
            } else {
                out.push_str(&format!(
                    "{indent}match consumer::request(\"tasks.worker{worker}\", b\"hello\", TIMEOUT_MS) {{\n\
                     {indent}    Ok(reply) => text.push_str(&format!(\n\
                     {indent}        \"worker{worker}: {{}}\\n\",\n\
                     {indent}        String::from_utf8_lossy(&reply.body)\n\
                     {indent}    )),\n\
                     {indent}    Err(err) => text.push_str(&format!(\"worker{worker} failed: {{err}}\\n\")),\n\
                     {indent}}}\n"
                ));
            }
        }
    }
    let invoke_await = if spec.edition.is_p3() { ".await" } else { "" };
    for import in &node.imports {
        let ident = rust_ident(import);
        // An async import takes owned values, so the p3 call converts.
        let argument = if spec.edition.is_p3() {
            "\"hello from the trigger\".to_string()"
        } else {
            "\"hello from the trigger\""
        };
        out.push_str(&format!(
            "{indent}match {ident}::invoke({argument}){invoke_await} {{\n\
             {indent}    Ok(value) => text.push_str(&format!(\"{ident}: {{value}}\\n\")),\n\
             {indent}    Err(err) => text.push_str(&format!(\"{ident} failed: {{err}}\\n\")),\n\
             {indent}}}\n"
        ));
    }
    for capability in &node.capabilities {
        let Some(helper) = capability_fn(capability) else {
            continue;
        };
        // `block_on` panics inside an existing reactor, so await-vs-block_on must not swap.
        let call = match (capability.is_async(spec.edition), is_async) {
            (true, true) => format!("{helper}().await"),
            (true, false) => format!("wstd::runtime::block_on({helper}())"),
            (false, _) => format!("{helper}()"),
        };
        out.push_str(&format!("{indent}text.push_str(&{call});\n"));
    }
    out
}

/// The HTTP trigger, as a wstd `http_server`.
fn http_trigger_main(spec: &Spec, node: &PlannedNode) -> String {
    let uses = use_lines(spec, node);
    let timeout = if spec.links_over_messaging() {
        "/// How long to wait for a worker to reply.\nconst TIMEOUT_MS: u32 = 5_000;\n\n"
    } else {
        ""
    };
    format!(
        "{uses}{timeout}\
         /// TODO: handle the request. Everything below is generated wiring.\n\
         #[wstd::http_server]\n\
         async fn main(\n    \
         _request: wstd::http::Request<wstd::http::Body>,\n\
         ) -> Result<wstd::http::Response<wstd::http::Body>, wstd::http::Error> {{\n    \
         #[allow(unused_mut)]\n    \
         let mut text = String::from(\"hello from wash wizard\\n\");\n\
         {calls}\n    \
         wstd::http::Response::builder()\n        \
         .status(wstd::http::StatusCode::OK)\n        \
         .body(wstd::http::Body::from(text))\n        \
         .map_err(Into::into)\n\
         }}\n",
        calls = downstream_calls(spec, node, "    ", true)
    )
}

/// The p3 service body: an async `wasi:cli/run@0.3.0` export sleeping on the monotonic clock.
fn service_run_impl(spec: &Spec, node: &PlannedNode) -> String {
    let calls = downstream_calls(spec, node, "        ", true);
    let preamble = if calls.is_empty() {
        String::new()
    } else {
        format!(
            "        let mut text = String::new();\n{calls}        eprintln!(\"{{text}}\");\n\n"
        )
    };
    format!(
        "impl bindings::exports::wasi::cli::run::Guest for Component {{\n    \
         /// TODO: do the long-lived work. The host starts this once and keeps\n    \
         /// it running; returning ends the run loop (though co-driven handler\n    \
         /// exports, if any, keep being served).\n    \
         async fn run() -> Result<(), ()> {{\n        \
         eprintln!(\"service started\");\n\n\
         {preamble}        \
         // Keep the service alive. Replace with a listener, a poll loop, or\n        \
         // whatever this service actually does.\n        \
         loop {{\n            \
         // 60 seconds, in nanoseconds.\n            \
         bindings::wasi::clocks::monotonic_clock::wait_for(60_000_000_000).await;\n        \
         }}\n    \
         }}\n\
         }}\n"
    )
}

/// A messaging handler: receives on its subject, calls downstream, replies when asked.
fn worker_impl(spec: &Spec, node: &PlannedNode) -> String {
    if spec.edition.is_p3() {
        return worker_impl_p3(spec, node);
    }
    // `text` is `mut` only when something appends, or the leaf case warns on unused `mut`.
    let appends = !node.imports.is_empty() || node.capabilities.iter().any(|c| c.emits_code());
    let binding = if appends { "let mut text" } else { "let text" };
    let mut body = format!(
        "        // TODO: replace with the real work.\n\
         \x20       let input = String::from_utf8_lossy(&message.body).to_string();\n\
         \x20       {binding} = format!(\"{{ID}} handled: {{input}}\\n\");\n"
    );
    body.push_str(&downstream_calls(spec, node, "        ", false));

    // A trigger has nobody waiting on it; a fan-out worker replies to the caller's `reply-to`.
    let reply = if node.is_trigger {
        String::from("        let _ = text;\n        Ok(())\n")
    } else {
        String::from(
            "        let Some(reply_to) = message.reply_to else {\n            \
             return Ok(());\n        \
             };\n        \
             consumer::publish(&BrokerMessage {\n            \
             subject: reply_to,\n            \
             body: text.into_bytes(),\n            \
             reply_to: None,\n        \
             })\n",
        )
    };

    format!(
        "const ID: &str = \"{id}\";\n\n\
         impl bindings::exports::wasmcloud::messaging::handler::Guest for Component {{\n    \
         fn handle_message(message: BrokerMessage) -> Result<(), String> {{\n\
         {body}{reply}    \
         }}\n\
         }}\n",
        id = node.id
    )
}

/// The `wasmcloud:messaging@0.3.0` worker: async `handle-message` with stream bodies.
fn worker_impl_p3(spec: &Spec, node: &PlannedNode) -> String {
    let appends = !node.imports.is_empty() || node.capabilities.iter().any(|c| c.emits_code());
    let binding = if appends { "let mut text" } else { "let text" };
    let mut body = format!(
        "        // The body is a native stream; drain it. A handler that does not\n\
         \x20       // care about the payload could simply drop the reader instead.\n\
         \x20       let input = String::from_utf8_lossy(&message.body.collect().await).to_string();\n\
         \x20       // TODO: replace with the real work.\n\
         \x20       {binding} = format!(\"{{ID}} handled: {{input}}\\n\");\n"
    );
    body.push_str(&downstream_calls(spec, node, "        ", true));

    let reply = if node.is_trigger {
        String::from("        let _ = text;\n        Ok(())\n")
    } else {
        String::from(
            "        let Some(reply_to) = message.reply_to else {\n            \
             return Ok(());\n        \
             };\n        \
             // Feed the reply from a spawned task; `publish` resolves only once\n        \
             // the host has fully consumed the stream.\n        \
             let (mut tx, reply_body) = bindings::wit_stream::new();\n        \
             wit_bindgen::spawn_local(async move {\n            \
             tx.write_all(text.into_bytes()).await;\n            \
             drop(tx);\n        \
             });\n        \
             consumer::publish(BrokerMessage {\n            \
             subject: reply_to,\n            \
             body: reply_body,\n            \
             reply_to: None,\n        \
             })\n        \
             .await\n        \
             .map_err(|err| HandleMessageError::Other(format!(\"reply failed: {err:?}\")))\n",
        )
    };

    format!(
        "const ID: &str = \"{id}\";\n\n\
         impl bindings::exports::wasmcloud::messaging::handler::Guest for Component {{\n    \
         async fn handle_message(message: BrokerMessage) -> Result<(), HandleMessageError> {{\n\
         {body}{reply}    \
         }}\n\
         }}\n",
        id = node.id
    )
}

/// Implementation of this component's own exported interface.
fn export_impl(spec: &Spec, node: &PlannedNode, export: &str) -> String {
    let ns = spec.wit_namespace_ident();
    let export_ident = rust_ident(export);

    let emits = node.capabilities.iter().any(|c| c.emits_code());
    let reassigns = !node.imports.is_empty() || emits;
    let binding = if reassigns {
        "let mut output"
    } else {
        "let output"
    };

    let mut body = format!(
        "        // TODO: replace with the real work.\n\
         \x20       {binding} = format!(\"{{ID}} handled: {{input}}\");\n"
    );
    // Under p3 `invoke` is `async func`; under p2 an async helper is driven with `block_on`.
    let invoke_await = if spec.edition.is_p3() { ".await" } else { "" };
    for import in &node.imports {
        let ident = rust_ident(import);
        // An async import takes an owned String; the p2 sync form borrows.
        let argument = if spec.edition.is_p3() {
            "output.clone()"
        } else {
            "&output"
        };
        body.push_str(&format!(
            "        match {ident}::invoke({argument}){invoke_await} {{\n            \
             Ok(value) => output = value,\n            \
             Err(err) => return Err(format!(\"{ident} failed: {{err}}\")),\n        \
             }}\n"
        ));
    }
    if emits {
        // `output` has no trailing newline; without this the first capability line runs on.
        body.push_str("        output.push('\\n');\n");
        for capability in &node.capabilities {
            let Some(helper) = capability_fn(capability) else {
                continue;
            };
            let call = match (capability.is_async(spec.edition), spec.edition.is_p3()) {
                (true, true) => format!("{helper}().await"),
                (true, false) => format!("wstd::runtime::block_on({helper}())"),
                (false, _) => format!("{helper}()"),
            };
            body.push_str(&format!("        output.push_str(&{call});\n"));
        }
    }

    let asyncness = if spec.edition.is_p3() { "async " } else { "" };
    format!(
        "const ID: &str = \"{id}\";\n\n\
         impl bindings::exports::{ns}::app::{export_ident}::Guest for Component {{\n    \
         {asyncness}fn invoke(input: String) -> Result<String, String> {{\n\
         {body}        \
         Ok(output)\n    \
         }}\n\
         }}\n",
        id = node.id
    )
}

// ---------------------------------------------------------------------------
// Project files
// ---------------------------------------------------------------------------

pub fn gitignore() -> String {
    String::from("target/\n")
}

/// Orientation for whoever opens the generated project.
pub fn readme(spec: &Spec) -> String {
    let nodes = spec.plan();
    let listing = nodes
        .iter()
        .map(|n| {
            let what = if n.is_trigger {
                format!("triggered when {}", spec.trigger.describe())
            } else if let Some(export) = &n.exports {
                format!("exports `{export}`")
            } else {
                format!("subscribes to `{}`", n.subscriptions.join(", "))
            };
            let has = if n.capabilities.is_empty() {
                String::new()
            } else {
                n.capabilities
                    .iter()
                    .map(|c| format!("`{}`", c.label()))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!("| `{}/` | {what} | {has} |", n.id)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let placed = spec.placed_capabilities();
    let mut capabilities = if placed.is_empty() {
        String::from("None.\n")
    } else {
        placed
            .iter()
            .map(|c| format!("- `{}` — {}\n", c.label(), c.describe()))
            .collect()
    };
    if placed.contains(&Capability::Secrets) {
        capabilities.push_str(
            "\n> **Secrets:** the value under `dev.host_interfaces` in \
             `.wash/config.yaml` is a development placeholder. Do not commit real \
             secrets there — production delivers them at bind time from a real \
             source, and the generated code only ever reveals a secret at the \
             moment of use.\n",
        );
    }

    format!(
        "# {name}\n\n\
         Generated by `wash wizard`. Triggered when {trigger}, and {linking}.\n\n\
         The wiring is complete — this builds and the host registers it before you \
         change anything. Every component body is a `TODO`.\n\n\
         ```sh\n\
         wash build\n\
         wash dev\n\
         ```\n\n\
         ## Components\n\n\
         | Directory | Role | Imports |\n|---|---|---|\n{listing}\n\n\
         ## Capabilities\n\n{capabilities}\n\
         ## How they are wired\n\n\
         Nothing in `.wash/config.yaml` connects these components. The host builds \
         a map of every interface exported across the workload and resolves each \
         component's imports against it, so the topology comes from `wit/world.wit` \
         alone.\n\n\
         That is why every direct edge has its **own** interface. An interface \
         exported by two components is ambiguous: the host drops it from the map and \
         no importer of it can link. Messaging workers are the exception — they \
         share one handler interface precisely because nothing imports it.\n",
        name = spec.name,
        trigger = spec.trigger.describe(),
        linking = spec.linking().describe(),
    )
}

#[cfg(test)]
mod tests {
    use super::super::spec::{Edition, Linking};
    use super::*;

    fn spec(trigger: Trigger, linking: Linking, count: usize) -> Spec {
        let (branches, over_messaging) = linking.expand(count);
        Spec {
            name: "demo".into(),
            trigger,
            edition: Edition::P2,
            branches,
            over_messaging,
            capabilities: Vec::new(),
            placement: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn every_capability_that_emits_code_defines_the_helper_it_calls() {
        for capability in [
            Capability::Keyvalue,
            Capability::Blobstore,
            Capability::Config,
            Capability::Logging,
            Capability::Postgres,
            Capability::HttpEgress {
                host: "example.com".into(),
            },
            Capability::Grpc {
                host: "example.com".into(),
            },
        ] {
            let name = capability_fn(&capability).expect("emits code");
            let helper = capability_helper(&capability, Edition::P2);
            assert!(
                helper.contains(&format!("fn {name}()")),
                "{name} helper missing its own definition:\n{helper}"
            );
        }
    }

    #[test]
    fn otel_contributes_config_only() {
        assert!(capability_fn(&Capability::Otel).is_none());
        for edition in Edition::ALL {
            assert!(capability_helper(&Capability::Otel, edition).is_empty());
            assert!(Capability::Otel.wit_imports(edition).is_empty());
        }
    }

    #[test]
    fn a_messaging_trigger_exports_the_handler_and_no_http() {
        let wit = world_wit(&spec(Trigger::Messaging, Linking::None, 1));
        assert!(
            wit.contains("export wasmcloud:messaging/handler@0.2.0;"),
            "{wit}"
        );
        assert!(!wit.contains("wasi:http"), "no HTTP anywhere:\n{wit}");
        // It only receives, so it must not import the consumer.
        assert!(!wit.contains("consumer"), "{wit}");
    }

    #[test]
    fn a_messaging_link_gives_the_trigger_the_consumer() {
        let wit = world_wit(&spec(Trigger::Http, Linking::Messaging, 2));
        assert!(
            wit.contains("import wasmcloud:messaging/consumer@0.2.0;"),
            "{wit}"
        );
        assert_eq!(
            wit.matches("export wasmcloud:messaging/handler@0.2.0;")
                .count(),
            2,
            "one per worker:\n{wit}"
        );
    }

    #[test]
    fn a_service_trigger_is_a_p3_cdylib_run_guest() {
        let mut s = spec(Trigger::Service, Linking::None, 1);
        s.edition = Edition::P3;
        let node = &s.plan()[0];

        let manifest = component_cargo_toml(&s, node);
        assert!(manifest.contains("crate-type = [\"cdylib\"]"), "{manifest}");
        assert!(!manifest.contains("wstd"), "p3 needs no wstd:\n{manifest}");

        let source = component_source(&s, node);
        assert!(source.contains("wit_bindgen::generate!"), "{source}");
        assert!(
            source.contains("impl bindings::exports::wasi::cli::run::Guest"),
            "{source}"
        );
        assert!(source.contains("async fn run()"), "{source}");
        assert!(source.contains("monotonic_clock::wait_for"), "{source}");
        assert!(!source.contains("#[wstd::main]"), "{source}");

        let wit = world_wit(&s);
        assert!(wit.contains("export wasi:cli/run@0.3.0;"), "{wit}");
        assert!(
            wit.contains("import wasi:clocks/monotonic-clock@0.3.0;"),
            "{wit}"
        );
    }

    #[test]
    fn an_http_trigger_never_declares_the_incoming_handler_itself() {
        // The raw wit-bindgen form fails to link against the messaging consumer.
        let wit = world_wit(&spec(Trigger::Http, Linking::None, 1));
        assert!(!wit.contains("wasi:http/incoming-handler"), "{wit}");
        let s = spec(Trigger::Http, Linking::None, 1);
        assert!(component_source(&s, &s.plan()[0]).contains("#[wstd::http_server]"));
    }

    #[test]
    fn grpc_ships_the_transport_adapter_rather_than_a_todo() {
        let helper = capability_helper(
            &Capability::Grpc {
                host: "grpc.example.com".into(),
            },
            Edition::P2,
        );
        assert!(helper.contains("impl tower_service::Service"), "{helper}");
        assert!(helper.contains("wstd::http::Client::new()"), "{helper}");
        assert!(helper.contains("client.invoke(request).await"), "{helper}");
    }

    #[test]
    fn an_async_capability_is_awaited_in_async_and_blocked_on_in_sync() {
        // `block_on` panics inside an existing reactor.
        let grpc = Capability::Grpc {
            host: "grpc.example.com".into(),
        };

        let mut async_ctx = spec(Trigger::Http, Linking::None, 1);
        async_ctx.capabilities = vec![grpc.clone(), Capability::Keyvalue];
        let source = component_source(&async_ctx, &async_ctx.plan()[0]);
        assert!(source.contains("call_grpc().await"), "{source}");
        assert!(!source.contains("block_on"), "{source}");
        assert!(
            !source.contains("touch_keyvalue().await"),
            "sync stays sync"
        );

        // A messaging trigger's `handle-message` is synchronous.
        let mut sync_ctx = spec(Trigger::Messaging, Linking::None, 1);
        sync_ctx.capabilities = vec![grpc];
        let source = component_source(&sync_ctx, &sync_ctx.plan()[0]);
        assert!(
            source.contains("wstd::runtime::block_on(call_grpc())"),
            "{source}"
        );
        assert!(!source.contains("call_grpc().await"), "{source}");
    }

    #[test]
    fn the_p3_trailers_future_defaults_to_the_type_the_response_wants() {
        // A bare `None` infers `Option<_>` and fails to compile in the emitted text.
        let mut s = spec(Trigger::Http, Linking::None, 1);
        s.edition = Edition::P3;
        let source = component_source(&s, &s.plan()[0]);

        assert!(source.contains("wit_future::new(|| Ok(None))"), "{source}");
        assert!(!source.contains("wit_future::new(|| None)"), "{source}");
    }

    #[test]
    fn grpc_pulls_in_the_proto_toolchain_only_where_it_is_used() {
        let mut s = spec(Trigger::Http, Linking::None, 1);
        s.capabilities = vec![Capability::Grpc {
            host: "grpc.example.com".into(),
        }];
        let workspace = workspace_cargo_toml(&s);
        assert!(workspace.contains("tonic-prost-build"), "{workspace}");

        let manifest = component_cargo_toml(&s, &s.plan()[0]);
        assert!(manifest.contains("[build-dependencies]"), "{manifest}");
        assert!(grpc_build_rs().contains("compile_protos"));
        assert!(grpc_proto().contains("service Wizard"));

        // A project without gRPC must not carry any of it.
        assert!(!workspace_cargo_toml(&spec(Trigger::Http, Linking::None, 1)).contains("tonic"));
    }

    #[test]
    fn capabilities_reach_both_the_world_and_the_source() {
        let mut s = spec(Trigger::Http, Linking::None, 1);
        s.capabilities = vec![Capability::Keyvalue, Capability::Logging];
        let wit = world_wit(&s);
        assert!(
            wit.contains("import wasi:keyvalue/store@0.2.0-draft;"),
            "{wit}"
        );
        assert!(
            wit.contains("import wasi:logging/logging@0.1.0-draft;"),
            "{wit}"
        );

        let source = component_source(&s, &s.plan()[0]);
        assert!(source.contains("touch_keyvalue()"), "{source}");
        assert!(source.contains("write_log()"), "{source}");
    }

    #[test]
    fn generated_source_never_unwraps_and_scopes_the_unsafe_allow() {
        let s = spec(Trigger::Http, Linking::Chain, 1);
        for node in &s.plan() {
            assert!(!component_source(&s, node).contains(".unwrap()"));
        }
        // Only components with a `Guest` impl need the export block.
        let callee = component_source(&s, &s.plan()[1]);
        assert!(callee.contains("#[allow(unsafe_code)]"), "{callee}");
    }

    #[test]
    fn the_generated_workspace_denies_the_same_lints_the_templates_do() {
        let manifest = workspace_cargo_toml(&spec(Trigger::Http, Linking::None, 1));
        for lint in ["unsafe_code", "unwrap_used", "panic", "indexing_slicing"] {
            assert!(manifest.contains(lint), "missing {lint} in:\n{manifest}");
        }
    }
}

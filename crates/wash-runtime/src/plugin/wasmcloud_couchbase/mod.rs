use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context as _, bail};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::instrument;
use url::Url;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::HostPlugin;
use crate::wit::{WitInterface, WitWorld};

const PLUGIN_COUCHBASE_ID: &str = "wasmcloud-couchbase";

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "couchbase",
        imports: { default: async | trappable | tracing },
    });
}

use bindings::wasmcloud::couchbase::document;
use bindings::wasmcloud::couchbase::types;

use document::DocumentError;

/// REST API response body for a document GET request.
#[derive(Deserialize)]
struct RestDocResponse {
    meta: RestDocMeta,
    json: serde_json::Value,
}

#[derive(Deserialize)]
struct RestDocMeta {
    rev: Option<String>,
}

/// Convert a Couchbase REST API revision string to a u64 CAS-like value.
///
/// The `rev` field has the form `seqno-<16-hex-digits>`, e.g. `1-000014b3a9200000`.
/// We extract the hex part and interpret it as a big-endian u64.
fn rev_to_cas(rev: Option<&str>) -> u64 {
    let Some(rev) = rev else {
        return 0;
    };
    if let Some(hex_part) = rev.split('-').nth(1) {
        let trimmed = &hex_part[..16.min(hex_part.len())];
        u64::from_str_radix(trimmed, 16).unwrap_or(0)
    } else {
        0
    }
}

/// wasmcloud:couchbase host plugin.
///
/// Routes component document operations to a Couchbase cluster via the
/// Couchbase Management REST API. The component provides the target bucket via
/// interface configuration; all other connection details are supplied when the
/// plugin is constructed.
///
/// # Notes on CAS
///
/// The Couchbase REST API does not expose the binary-protocol CAS for
/// conditional mutations. The `cas` fields in the WIT interface are included
/// for forward-compatibility with a future binary-protocol implementation.
/// This REST-based implementation derives a best-effort CAS from the `rev`
/// field returned on GET, but does not enforce it on write operations.
#[derive(Clone)]
pub struct WasmcloudCouchbase {
    /// Cluster base URL without trailing slash, e.g. `http://host:8091`
    base_url: String,
    /// Cluster admin / bucket username
    username: String,
    /// Cluster admin / bucket password
    password: String,
    /// Shared HTTP client (cheap to clone — Arc-backed internally)
    client: Client,
    /// component_id → bucket_name
    component_buckets: Arc<RwLock<HashMap<String, String>>>,
}

impl WasmcloudCouchbase {
    /// Create a new `WasmcloudCouchbase` plugin.
    ///
    /// The URL must include credentials and point at the Couchbase management
    /// port (default 8091):
    ///
    /// ```text
    /// http://Administrator:password@localhost:8091
    /// ```
    pub fn new(url: &str) -> anyhow::Result<Self> {
        let parsed = Url::parse(url).context("failed to parse Couchbase URL")?;
        let username = parsed.username().to_string();
        let password = parsed.password().unwrap_or("").to_string();

        let mut base = parsed.clone();
        base.set_path("");
        base.set_query(None);
        base.set_username("").ok();
        base.set_password(None).ok();

        let base_url = base.to_string().trim_end_matches('/').to_string();

        let client = Client::builder()
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self {
            base_url,
            username,
            password,
            client,
            component_buckets: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Build the REST URL for a specific document.
    fn doc_url(&self, bucket: &str, key: &str) -> anyhow::Result<String> {
        let mut url = Url::parse(&self.base_url).context("invalid Couchbase base URL")?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Couchbase base URL cannot be a base URL"))?
            .extend(["pools", "default", "buckets", bucket, "docs", key]);
        Ok(url.to_string())
    }

    /// Retrieve the configured bucket name for a component.
    async fn bucket_for_component(&self, component_id: &str) -> Option<String> {
        self.component_buckets
            .read()
            .await
            .get(component_id)
            .cloned()
    }
}

// ── Shared helper ────────────────────────────────────────────────────────────

/// POST document content to the Couchbase REST API (upsert semantics).
///
/// Returns the CAS of the stored document. The REST API does not expose the
/// binary-protocol CAS on writes, so this returns 0.
async fn store_document(
    plugin: &WasmcloudCouchbase,
    url: &str,
    content: &str,
    expiry_secs: u32,
) -> wasmtime::Result<Result<u64, DocumentError>> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("value", content)
        .append_pair("flags", "0")
        .append_pair("expiry", &expiry_secs.to_string())
        .finish();

    let resp = match plugin
        .client
        .post(url)
        .basic_auth(&plugin.username, Some(&plugin.password))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Ok(Err(DocumentError::Unexpected(format!(
                "request failed: {e}"
            ))));
        }
    };

    match resp.status() {
        s if s.is_success() => Ok(Ok(0)),
        StatusCode::BAD_REQUEST => Ok(Err(DocumentError::Unexpected(
            "bad request — check that content is valid JSON".to_string(),
        ))),
        s => Ok(Err(DocumentError::Unexpected(format!(
            "unexpected HTTP status: {s}"
        )))),
    }
}

// ── Host trait implementations ───────────────────────────────────────────────

impl<'a> types::Host for ActiveCtx<'a> {}

impl<'a> document::Host for ActiveCtx<'a> {
    #[instrument(skip_all, fields(key = %key))]
    async fn get(
        &mut self,
        key: String,
    ) -> wasmtime::Result<Result<document::Document, DocumentError>> {
        let Some(plugin) = self.get_plugin::<WasmcloudCouchbase>(PLUGIN_COUCHBASE_ID) else {
            return Ok(Err(DocumentError::Unexpected(
                "couchbase plugin not available".to_string(),
            )));
        };

        let component_id = self.component_id.to_string();
        let bucket = match plugin.bucket_for_component(&component_id).await {
            Some(b) => b,
            None => {
                return Ok(Err(DocumentError::Unexpected(
                    "no bucket configured for this component".to_string(),
                )));
            }
        };

        let url = match plugin.doc_url(&bucket, &key) {
            Ok(u) => u,
            Err(e) => {
                return Ok(Err(DocumentError::Unexpected(format!(
                    "failed to build URL: {e}"
                ))));
            }
        };

        let resp = match plugin
            .client
            .get(&url)
            .basic_auth(&plugin.username, Some(&plugin.password))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(Err(DocumentError::Unexpected(format!(
                    "request failed: {e}"
                ))));
            }
        };

        match resp.status() {
            StatusCode::OK => {
                let body: RestDocResponse = match resp.json().await {
                    Ok(b) => b,
                    Err(e) => {
                        return Ok(Err(DocumentError::Unexpected(format!(
                            "failed to parse response: {e}"
                        ))));
                    }
                };
                Ok(Ok(document::Document {
                    key,
                    content: body.json.to_string(),
                    cas: rev_to_cas(body.meta.rev.as_deref()),
                }))
            }
            StatusCode::NOT_FOUND => Ok(Err(DocumentError::NotFound)),
            s => Ok(Err(DocumentError::Unexpected(format!(
                "unexpected HTTP status: {s}"
            )))),
        }
    }

    #[instrument(skip_all, fields(key = %key))]
    async fn exists(&mut self, key: String) -> wasmtime::Result<Result<bool, DocumentError>> {
        let Some(plugin) = self.get_plugin::<WasmcloudCouchbase>(PLUGIN_COUCHBASE_ID) else {
            return Ok(Err(DocumentError::Unexpected(
                "couchbase plugin not available".to_string(),
            )));
        };

        let component_id = self.component_id.to_string();
        let bucket = match plugin.bucket_for_component(&component_id).await {
            Some(b) => b,
            None => {
                return Ok(Err(DocumentError::Unexpected(
                    "no bucket configured for this component".to_string(),
                )));
            }
        };

        let url = match plugin.doc_url(&bucket, &key) {
            Ok(u) => u,
            Err(e) => {
                return Ok(Err(DocumentError::Unexpected(format!(
                    "failed to build URL: {e}"
                ))));
            }
        };

        let resp = match plugin
            .client
            .get(&url)
            .basic_auth(&plugin.username, Some(&plugin.password))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(Err(DocumentError::Unexpected(format!(
                    "request failed: {e}"
                ))));
            }
        };

        match resp.status() {
            StatusCode::OK => Ok(Ok(true)),
            StatusCode::NOT_FOUND => Ok(Ok(false)),
            s => Ok(Err(DocumentError::Unexpected(format!(
                "unexpected HTTP status: {s}"
            )))),
        }
    }

    #[instrument(skip_all, fields(key = %key))]
    async fn insert(
        &mut self,
        key: String,
        content: String,
        expiry_secs: u32,
    ) -> wasmtime::Result<Result<u64, DocumentError>> {
        let Some(plugin) = self.get_plugin::<WasmcloudCouchbase>(PLUGIN_COUCHBASE_ID) else {
            return Ok(Err(DocumentError::Unexpected(
                "couchbase plugin not available".to_string(),
            )));
        };

        let component_id = self.component_id.to_string();
        let bucket = match plugin.bucket_for_component(&component_id).await {
            Some(b) => b,
            None => {
                return Ok(Err(DocumentError::Unexpected(
                    "no bucket configured for this component".to_string(),
                )));
            }
        };

        let url = match plugin.doc_url(&bucket, &key) {
            Ok(u) => u,
            Err(e) => {
                return Ok(Err(DocumentError::Unexpected(format!(
                    "failed to build URL: {e}"
                ))));
            }
        };

        // The REST API has no atomic insert; check existence first (best effort).
        let head_resp = match plugin
            .client
            .get(&url)
            .basic_auth(&plugin.username, Some(&plugin.password))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(Err(DocumentError::Unexpected(format!(
                    "existence check failed: {e}"
                ))));
            }
        };

        if head_resp.status() == StatusCode::OK {
            return Ok(Err(DocumentError::AlreadyExists));
        }

        store_document(&plugin, &url, &content, expiry_secs).await
    }

    #[instrument(skip_all, fields(key = %key))]
    async fn upsert(
        &mut self,
        key: String,
        content: String,
        expiry_secs: u32,
    ) -> wasmtime::Result<Result<u64, DocumentError>> {
        let Some(plugin) = self.get_plugin::<WasmcloudCouchbase>(PLUGIN_COUCHBASE_ID) else {
            return Ok(Err(DocumentError::Unexpected(
                "couchbase plugin not available".to_string(),
            )));
        };

        let component_id = self.component_id.to_string();
        let bucket = match plugin.bucket_for_component(&component_id).await {
            Some(b) => b,
            None => {
                return Ok(Err(DocumentError::Unexpected(
                    "no bucket configured for this component".to_string(),
                )));
            }
        };

        let url = match plugin.doc_url(&bucket, &key) {
            Ok(u) => u,
            Err(e) => {
                return Ok(Err(DocumentError::Unexpected(format!(
                    "failed to build URL: {e}"
                ))));
            }
        };

        store_document(&plugin, &url, &content, expiry_secs).await
    }

    #[instrument(skip_all, fields(key = %key))]
    async fn replace(
        &mut self,
        key: String,
        content: String,
        _cas: u64,
        expiry_secs: u32,
    ) -> wasmtime::Result<Result<u64, DocumentError>> {
        let Some(plugin) = self.get_plugin::<WasmcloudCouchbase>(PLUGIN_COUCHBASE_ID) else {
            return Ok(Err(DocumentError::Unexpected(
                "couchbase plugin not available".to_string(),
            )));
        };

        let component_id = self.component_id.to_string();
        let bucket = match plugin.bucket_for_component(&component_id).await {
            Some(b) => b,
            None => {
                return Ok(Err(DocumentError::Unexpected(
                    "no bucket configured for this component".to_string(),
                )));
            }
        };

        let url = match plugin.doc_url(&bucket, &key) {
            Ok(u) => u,
            Err(e) => {
                return Ok(Err(DocumentError::Unexpected(format!(
                    "failed to build URL: {e}"
                ))));
            }
        };

        // Verify the document exists before replacing.
        let check_resp = match plugin
            .client
            .get(&url)
            .basic_auth(&plugin.username, Some(&plugin.password))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(Err(DocumentError::Unexpected(format!(
                    "existence check failed: {e}"
                ))));
            }
        };

        if check_resp.status() == StatusCode::NOT_FOUND {
            return Ok(Err(DocumentError::NotFound));
        }

        store_document(&plugin, &url, &content, expiry_secs).await
    }

    #[instrument(skip_all, fields(key = %key))]
    async fn remove(
        &mut self,
        key: String,
        _cas: u64,
    ) -> wasmtime::Result<Result<(), DocumentError>> {
        let Some(plugin) = self.get_plugin::<WasmcloudCouchbase>(PLUGIN_COUCHBASE_ID) else {
            return Ok(Err(DocumentError::Unexpected(
                "couchbase plugin not available".to_string(),
            )));
        };

        let component_id = self.component_id.to_string();
        let bucket = match plugin.bucket_for_component(&component_id).await {
            Some(b) => b,
            None => {
                return Ok(Err(DocumentError::Unexpected(
                    "no bucket configured for this component".to_string(),
                )));
            }
        };

        let url = match plugin.doc_url(&bucket, &key) {
            Ok(u) => u,
            Err(e) => {
                return Ok(Err(DocumentError::Unexpected(format!(
                    "failed to build URL: {e}"
                ))));
            }
        };

        let resp = match plugin
            .client
            .delete(&url)
            .basic_auth(&plugin.username, Some(&plugin.password))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(Err(DocumentError::Unexpected(format!(
                    "request failed: {e}"
                ))));
            }
        };

        match resp.status() {
            s if s.is_success() => Ok(Ok(())),
            StatusCode::NOT_FOUND => Ok(Err(DocumentError::NotFound)),
            s => Ok(Err(DocumentError::Unexpected(format!(
                "unexpected HTTP status: {s}"
            )))),
        }
    }
}

// ── HostPlugin implementation ────────────────────────────────────────────────

#[async_trait::async_trait]
impl HostPlugin for WasmcloudCouchbase {
    fn id(&self) -> &'static str {
        PLUGIN_COUCHBASE_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasmcloud:couchbase/types,document@0.1.0-draft",
            )]),
            ..Default::default()
        }
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        let bucket = match interfaces
            .iter()
            .find(|i| i.namespace == "wasmcloud" && i.package == "couchbase")
        {
            Some(i) => match i.config.get("bucket").cloned() {
                Some(b) => b,
                None => bail!("wasmcloud:couchbase requires a 'bucket' config parameter"),
            },
            None => return Ok(()),
        };

        let component_id = component_handle.id().to_string();

        tracing::debug!(
            component_id = %component_id,
            bucket = %bucket,
            "Binding couchbase plugin to component"
        );

        self.component_buckets
            .write()
            .await
            .insert(component_id, bucket);

        let linker = component_handle.linker();
        bindings::wasmcloud::couchbase::types::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        bindings::wasmcloud::couchbase::document::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        tracing::debug!(workload_id = %workload_id, "Unbinding couchbase plugin from workload");

        self.component_buckets
            .write()
            .await
            .retain(|k, _| !k.starts_with(workload_id));

        Ok(())
    }
}

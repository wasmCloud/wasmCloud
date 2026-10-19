//! A read-only OCI distribution registry holding one wasm component.
//!
//! Hosts take components only by OCI reference, so the bench serves the one
//! it built. It answers exactly what oci-client 0.17 (the wasmCloud host's
//! client) asks for in a pull:
//!
//! - `GET /v2/`: the auth probe; no `WWW-Authenticate` means anonymous.
//! - `GET /v2/<name>/manifests/<tag|digest>`: an OCI image manifest. The
//!   client checks `schemaVersion` and `mediaType` and hashes the body against
//!   `Docker-Content-Digest`.
//! - `HEAD /v2/<name>/manifests/<tag|digest>`: the digest-only check a host
//!   with a cached copy makes before reusing it.
//! - `GET /v2/<name>/blobs/<digest>`: the config, then the layer, each hashed
//!   against its descriptor (and `Docker-Content-Digest`, when sent).
//!
//! The layer media type is `application/wasm`, the one the host accepts.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result};
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

pub const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
pub const CONFIG_MEDIA_TYPE: &str = "application/vnd.wasm.config.v0+json";
pub const LAYER_MEDIA_TYPE: &str = "application/wasm";

/// `sha256:<hex>` of `bytes`.
pub fn sha256_digest(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for byte in hash {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor<'a> {
    media_type: &'a str,
    digest: &'a str,
    size: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest<'a> {
    schema_version: u32,
    media_type: &'a str,
    config: Descriptor<'a>,
    layers: [Descriptor<'a>; 1],
}

/// The wasm OCI artifact config, after oci-wasm's shape, kept fixed so the
/// manifest digest depends on the component alone.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmConfig<'a> {
    created: &'a str,
    architecture: &'a str,
    os: &'a str,
    layer_digests: [&'a str; 1],
}

/// One component as an OCI artifact: manifest, config and layer blobs.
#[derive(Debug, Clone)]
pub struct Image {
    pub repository: String,
    pub tag: String,
    pub manifest: Bytes,
    pub manifest_digest: String,
    pub config: Bytes,
    pub config_digest: String,
    pub layer: Bytes,
    pub layer_digest: String,
}

impl Image {
    /// Package `wasm` under `repository`, tagged by its content hash.
    pub fn new(repository: &str, wasm: Vec<u8>) -> Result<Self> {
        let layer_digest = sha256_digest(&wasm);
        let config = serde_json::to_vec(&WasmConfig {
            created: "1970-01-01T00:00:00Z",
            architecture: "wasm",
            os: "wasip3",
            layer_digests: [&layer_digest],
        })?;
        let config_digest = sha256_digest(&config);
        let manifest = serde_json::to_vec(&Manifest {
            schema_version: 2,
            media_type: MANIFEST_MEDIA_TYPE,
            config: Descriptor {
                media_type: CONFIG_MEDIA_TYPE,
                digest: &config_digest,
                size: config.len(),
            },
            layers: [Descriptor {
                media_type: LAYER_MEDIA_TYPE,
                digest: &layer_digest,
                size: wasm.len(),
            }],
        })?;
        let manifest_digest = sha256_digest(&manifest);
        let tag = layer_digest
            .strip_prefix("sha256:")
            .and_then(|hex| hex.get(..16))
            .context("digest too short")?
            .to_string();
        Ok(Self {
            repository: repository.to_string(),
            tag,
            manifest: manifest.into(),
            manifest_digest,
            config: config.into(),
            config_digest,
            layer: wasm.into(),
            layer_digest,
        })
    }

    pub fn reference(&self, addr: SocketAddr) -> String {
        format!("{addr}/{}:{}", self.repository, self.tag)
    }
}

/// Requests the registry answered, for the report: they show whether the
/// host pulled once or on every start.
#[derive(Debug, Default, Serialize, Clone, Copy)]
pub struct RegistryCounts {
    pub manifest_gets: u64,
    pub manifest_heads: u64,
    pub blob_gets: u64,
    pub not_found: u64,
}

#[derive(Default)]
struct Counters {
    manifest_gets: AtomicU64,
    manifest_heads: AtomicU64,
    blob_gets: AtomicU64,
    not_found: AtomicU64,
}

/// What a request resolved to; the pure half of the server, so tests can
/// check the routing without a socket.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    Version,
    Manifest,
    Blob(Blob),
    NotFound(&'static str),
    MethodNotAllowed,
}

#[derive(Debug, PartialEq, Eq)]
enum Blob {
    Config,
    Layer,
}

fn route(image: &Image, method: &Method, path: &str) -> Route {
    if method != Method::GET && method != Method::HEAD {
        return Route::MethodNotAllowed;
    }
    if path == "/v2/" || path == "/v2" {
        return Route::Version;
    }
    let Some(rest) = path.strip_prefix("/v2/") else {
        return Route::NotFound("NAME_UNKNOWN");
    };
    if let Some((name, reference)) = rest.rsplit_once("/manifests/") {
        if name != image.repository {
            return Route::NotFound("NAME_UNKNOWN");
        }
        if reference == image.tag || reference == image.manifest_digest {
            return Route::Manifest;
        }
        return Route::NotFound("MANIFEST_UNKNOWN");
    }
    if let Some((name, digest)) = rest.rsplit_once("/blobs/") {
        if name != image.repository {
            return Route::NotFound("NAME_UNKNOWN");
        }
        if digest == image.config_digest {
            return Route::Blob(Blob::Config);
        }
        if digest == image.layer_digest {
            return Route::Blob(Blob::Layer);
        }
        return Route::NotFound("BLOB_UNKNOWN");
    }
    Route::NotFound("NAME_UNKNOWN")
}

fn respond(
    image: &Image,
    counters: &Counters,
    method: &Method,
    path: &str,
) -> Response<Full<Bytes>> {
    let head = method == Method::HEAD;
    let found = |content_type: &str, digest: &str, body: &Bytes| {
        let builder = Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", content_type)
            .header("Content-Length", body.len())
            .header("Docker-Content-Digest", digest);
        let body = if head { Bytes::new() } else { body.clone() };
        builder.body(Full::new(body))
    };
    let response = match route(image, method, path) {
        Route::Version => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .header("Docker-Distribution-API-Version", "registry/2.0")
            .body(Full::new(Bytes::from_static(b"{}"))),
        Route::Manifest => {
            let counter = if head {
                &counters.manifest_heads
            } else {
                &counters.manifest_gets
            };
            counter.fetch_add(1, Ordering::Relaxed);
            found(MANIFEST_MEDIA_TYPE, &image.manifest_digest, &image.manifest)
        }
        Route::Blob(blob) => {
            counters.blob_gets.fetch_add(1, Ordering::Relaxed);
            match blob {
                Blob::Config => found(
                    "application/octet-stream",
                    &image.config_digest,
                    &image.config,
                ),
                Blob::Layer => found(
                    "application/octet-stream",
                    &image.layer_digest,
                    &image.layer,
                ),
            }
        }
        Route::NotFound(code) => {
            counters.not_found.fetch_add(1, Ordering::Relaxed);
            let body =
                serde_json::json!({"errors": [{"code": code, "message": path, "detail": null}]});
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(body.to_string())))
        }
        Route::MethodNotAllowed => Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Full::new(Bytes::from_static(b"read-only registry"))),
    };
    response.unwrap_or_else(|_| {
        let mut fallback = Response::new(Full::new(Bytes::new()));
        *fallback.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        fallback
    })
}

/// A running registry; dropping it stops serving.
pub struct Registry {
    pub addr: SocketAddr,
    counters: Arc<Counters>,
    accept: tokio::task::JoinHandle<()>,
}

impl Registry {
    pub async fn serve(image: Arc<Image>) -> Result<Self> {
        Self::serve_with_limits(image, 256, std::time::Duration::from_secs(120)).await
    }

    async fn serve_with_limits(
        image: Arc<Image>,
        max_connections: usize,
        lifetime: std::time::Duration,
    ) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding the registry")?;
        let addr = listener.local_addr()?;
        let counters = Arc::new(Counters::default());
        let accept = tokio::spawn({
            let counters = Arc::clone(&counters);
            async move {
                // The set bounds admission and owns every accepted connection.
                // Aborting this accept task drops the set, closing its sockets too.
                let mut connections = tokio::task::JoinSet::new();
                loop {
                    let stream = tokio::select! {
                        Some(_) = connections.join_next(), if !connections.is_empty() => continue,
                        accepted = listener.accept() => match accepted {
                            Ok((stream, _)) => stream,
                            Err(_) => break,
                        },
                    };
                    if connections.len() >= max_connections {
                        continue;
                    }
                    let image = Arc::clone(&image);
                    let counters = Arc::clone(&counters);
                    connections.spawn(async move {
                        let service =
                            hyper::service::service_fn(move |request: Request<Incoming>| {
                                let response = respond(
                                    &image,
                                    &counters,
                                    request.method(),
                                    request.uri().path(),
                                );
                                async move { Ok::<_, Infallible>(response) }
                            });
                        // A pull that hangs up early is the client's business.
                        let mut builder = hyper::server::conn::http1::Builder::new();
                        builder
                            .timer(hyper_util::rt::TokioTimer::new())
                            .header_read_timeout(std::time::Duration::from_secs(10));
                        let _ = tokio::time::timeout(
                            lifetime,
                            builder.serve_connection(hyper_util::rt::TokioIo::new(stream), service),
                        )
                        .await;
                    });
                }
            }
        });
        Ok(Self {
            addr,
            counters,
            accept,
        })
    }

    pub fn counts(&self) -> RegistryCounts {
        RegistryCounts {
            manifest_gets: self.counters.manifest_gets.load(Ordering::Relaxed),
            manifest_heads: self.counters.manifest_heads.load(Ordering::Relaxed),
            blob_gets: self.counters.blob_gets.load(Ordering::Relaxed),
            not_found: self.counters.not_found.load(Ordering::Relaxed),
        }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    async fn connected_registry(addr: SocketAddr) -> tokio::net::TcpStream {
        let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
        socket
            .write_all(b"GET /v2/ HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n{}") {
            response.push(socket.read_u8().await.unwrap());
            assert!(response.len() < 4096);
        }
        socket
    }

    async fn assert_closed(mut socket: tokio::net::TcpStream) {
        let read = tokio::time::timeout(std::time::Duration::from_secs(5), socket.read_u8())
            .await
            .expect("registry connection must close");
        assert!(read.is_err(), "expected EOF or a connection reset");
    }

    #[tokio::test]
    async fn connections_are_bounded_and_closed_when_registry_drops() {
        let registry =
            Registry::serve_with_limits(Arc::new(image()), 2, std::time::Duration::from_secs(120))
                .await
                .unwrap();
        let first = connected_registry(registry.addr).await;
        let second = connected_registry(registry.addr).await;
        let excess = tokio::net::TcpStream::connect(registry.addr).await.unwrap();
        assert_closed(excess).await;
        drop(registry);
        assert_closed(first).await;
        assert_closed(second).await;
    }

    #[tokio::test]
    async fn accepted_connections_have_a_finite_lifetime() {
        let registry =
            Registry::serve_with_limits(Arc::new(image()), 1, std::time::Duration::from_secs(60))
                .await
                .unwrap();
        let socket = connected_registry(registry.addr).await;
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(61)).await;
        tokio::time::resume();
        assert_closed(socket).await;
    }

    fn image() -> Image {
        Image::new(
            "bench/http-handler",
            b"\0asm\x0d\0\x01\0fake component".to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn digests_are_sha256_of_the_bytes() {
        assert_eq!(
            sha256_digest(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let image = image();
        assert_eq!(image.layer_digest, sha256_digest(&image.layer));
        assert_eq!(image.config_digest, sha256_digest(&image.config));
        assert_eq!(image.manifest_digest, sha256_digest(&image.manifest));
        assert_eq!(image.tag.len(), 16);
        assert!(image.layer_digest.contains(&image.tag));
    }

    #[test]
    fn manifest_is_an_oci_wasm_image_manifest() {
        let image = image();
        let manifest: serde_json::Value = serde_json::from_slice(&image.manifest).unwrap();
        assert_eq!(manifest["schemaVersion"], 2);
        assert_eq!(manifest["mediaType"], MANIFEST_MEDIA_TYPE);
        assert_eq!(manifest["config"]["mediaType"], CONFIG_MEDIA_TYPE);
        assert_eq!(manifest["config"]["digest"], image.config_digest.as_str());
        assert_eq!(manifest["config"]["size"], image.config.len());
        let layers = manifest["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0]["mediaType"], LAYER_MEDIA_TYPE);
        assert_eq!(layers[0]["digest"], image.layer_digest.as_str());
        assert_eq!(layers[0]["size"], image.layer.len());
        let config: serde_json::Value = serde_json::from_slice(&image.config).unwrap();
        assert_eq!(config["architecture"], "wasm");
        assert_eq!(config["layerDigests"][0], image.layer_digest.as_str());
    }

    #[test]
    fn same_component_same_reference() {
        assert_eq!(image().manifest_digest, image().manifest_digest);
        let other = Image::new("bench/http-handler", b"\0asm other".to_vec()).unwrap();
        assert_ne!(other.tag, image().tag);
    }

    #[test]
    fn routes_what_oci_client_asks_for() {
        let image = image();
        let get = Method::GET;
        assert_eq!(route(&image, &get, "/v2/"), Route::Version);
        let by_tag = format!("/v2/bench/http-handler/manifests/{}", image.tag);
        assert_eq!(route(&image, &get, &by_tag), Route::Manifest);
        assert_eq!(route(&image, &Method::HEAD, &by_tag), Route::Manifest);
        let by_digest = format!("/v2/bench/http-handler/manifests/{}", image.manifest_digest);
        assert_eq!(route(&image, &get, &by_digest), Route::Manifest);
        let config = format!("/v2/bench/http-handler/blobs/{}", image.config_digest);
        assert_eq!(route(&image, &get, &config), Route::Blob(Blob::Config));
        let layer = format!("/v2/bench/http-handler/blobs/{}", image.layer_digest);
        assert_eq!(route(&image, &get, &layer), Route::Blob(Blob::Layer));
        assert_eq!(
            route(&image, &get, "/v2/bench/http-handler/manifests/latest"),
            Route::NotFound("MANIFEST_UNKNOWN")
        );
        assert_eq!(
            route(&image, &get, &format!("/v2/other/manifests/{}", image.tag)),
            Route::NotFound("NAME_UNKNOWN")
        );
        assert_eq!(
            route(&image, &Method::PUT, &by_tag),
            Route::MethodNotAllowed
        );
    }

    #[test]
    fn responses_carry_the_digest_header() {
        let image = image();
        let counters = Counters::default();
        let path = format!("/v2/bench/http-handler/manifests/{}", image.tag);
        let response = respond(&image, &counters, &Method::HEAD, &path);
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(
            headers["Docker-Content-Digest"],
            image.manifest_digest.as_str()
        );
        assert_eq!(headers["Content-Type"], MANIFEST_MEDIA_TYPE);
        assert_eq!(
            headers["Content-Length"],
            image.manifest.len().to_string().as_str()
        );
        assert_eq!(counters.manifest_heads.load(Ordering::Relaxed), 1);
    }
}

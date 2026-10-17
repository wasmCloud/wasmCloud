//! Cache policy and authentication against a loopback OCI registry.
#![cfg(feature = "oci")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use wash_runtime::oci::{OciConfig, OciPullPolicy, pull_component};

struct Image {
    manifest: Vec<u8>,
    digest: String,
    config: Vec<u8>,
    config_digest: String,
    wasm: Vec<u8>,
    wasm_digest: String,
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

impl Image {
    fn new(wasm: &str) -> Self {
        let wasm = wat::parse_str(wasm).unwrap();
        let wasm_digest = digest(&wasm);
        let config = serde_json::to_vec(&serde_json::json!({
            "architecture": "wasm", "os": "wasip2", "layerDigests": [&wasm_digest]
        }))
        .unwrap();
        let config_digest = digest(&config);
        let manifest = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.wasm.config.v0+json",
                "digest": &config_digest, "size": config.len()
            },
            "layers": [{
                "mediaType": "application/wasm", "digest": &wasm_digest, "size": wasm.len()
            }]
        }))
        .unwrap();
        Self {
            digest: digest(&manifest),
            manifest,
            config,
            config_digest,
            wasm,
            wasm_digest,
        }
    }
}

struct Registry {
    addr: std::net::SocketAddr,
    image: Arc<RwLock<Image>>,
    unavailable: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Registry {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let image = Arc::new(RwLock::new(Image::new("(component)")));
        let unavailable = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn({
            let image = Arc::clone(&image);
            let requests = Arc::clone(&requests);
            let unavailable = Arc::clone(&unavailable);
            async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let image = Arc::clone(&image);
                    let requests = Arc::clone(&requests);
                    let unavailable = Arc::clone(&unavailable);
                    tokio::spawn(async move {
                        let service = service_fn(move |req| {
                            requests.fetch_add(1, Ordering::SeqCst);
                            let response = if unavailable.load(Ordering::SeqCst) {
                                Response::builder()
                                    .status(StatusCode::SERVICE_UNAVAILABLE)
                                    .body(Full::new(Bytes::new()))
                                    .unwrap()
                            } else {
                                answer(req, &image.read().unwrap())
                            };
                            std::future::ready(Ok::<_, Infallible>(response))
                        });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            }
        });
        Self {
            addr,
            image,
            unavailable,
            requests,
            task,
        }
    }

    fn reference(&self) -> String {
        format!(
            "{}/private/component@{}",
            self.addr,
            self.image.read().unwrap().digest
        )
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn answer(req: Request<Incoming>, image: &Image) -> Response<Full<Bytes>> {
    let mut response = Response::builder().header("connection", "close");
    if req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        != Some("Basic dXNlcjpzZWNyZXQ=")
    {
        return response
            .status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate", "Basic realm=\"test-registry\"")
            .body(Full::new(Bytes::new()))
            .unwrap();
    }
    let path = req.uri().path();
    let body = if path == "/v2/" {
        b"{}".to_vec()
    } else if path.contains("/manifests/") {
        response = response
            .header("content-type", "application/vnd.oci.image.manifest.v1+json")
            .header("docker-content-digest", &image.digest);
        image.manifest.clone()
    } else if path.ends_with(&image.config_digest) {
        image.config.clone()
    } else if path.ends_with(&image.wasm_digest) {
        image.wasm.clone()
    } else {
        response = response.status(StatusCode::NOT_FOUND);
        Vec::new()
    };
    response
        .header("content-length", body.len())
        .body(Full::new(body.into()))
        .unwrap()
}

#[tokio::test]
async fn private_cached_images_do_not_cross_credentials() {
    wash_runtime::init_crypto();
    let registry = Registry::start().await;
    let cache = tempfile::tempdir().unwrap();
    let config = OciConfig {
        credentials: Some(("user".into(), "secret".into())),
        cache_dir: Some(cache.path().to_owned()),
        insecure: true,
        timeout: Some(Duration::from_secs(5)),
    };
    for reference in [
        registry.reference(),
        format!("{}/private/component:v1", registry.addr),
    ] {
        let (bytes, _) = pull_component(&reference, config.clone(), OciPullPolicy::IfNotPresent)
            .await
            .unwrap();
        assert_eq!(bytes, registry.image.read().unwrap().wasm);
        let fetched = registry.requests.load(Ordering::SeqCst);
        pull_component(&reference, config.clone(), OciPullPolicy::Never)
            .await
            .unwrap();
        assert_eq!(
            registry.requests.load(Ordering::SeqCst),
            fetched,
            "Never is offline"
        );

        for credentials in [Some(("user".into(), "wrong".into())), None] {
            let rejected = OciConfig {
                credentials,
                ..config.clone()
            };
            assert!(
                pull_component(&reference, rejected.clone(), OciPullPolicy::Never)
                    .await
                    .is_err()
            );
            assert!(
                pull_component(&reference, rejected, OciPullPolicy::IfNotPresent)
                    .await
                    .is_err()
            );
        }
    }
}

#[tokio::test]
async fn always_resolves_repushed_tags_and_retries_completed_failures() {
    wash_runtime::init_crypto();
    let registry = Registry::start().await;
    let config = OciConfig {
        credentials: Some(("user".into(), "secret".into())),
        insecure: true,
        timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    };
    let reference = format!("{}/private/component:latest", registry.addr);
    let first = pull_component(&reference, config.clone(), OciPullPolicy::Always)
        .await
        .unwrap();
    *registry.image.write().unwrap() = Image::new("(component (core module))");
    let second = pull_component(&reference, config.clone(), OciPullPolicy::Always)
        .await
        .unwrap();
    assert_ne!(
        first.1, second.1,
        "a completed Always pull must resolve the tag again"
    );
    assert_ne!(first.0, second.0);

    registry.unavailable.store(true, Ordering::SeqCst);
    assert!(
        pull_component(&reference, config.clone(), OciPullPolicy::Always)
            .await
            .is_err()
    );
    registry.unavailable.store(false, Ordering::SeqCst);
    assert!(
        pull_component(&reference, config, OciPullPolicy::Always)
            .await
            .is_ok()
    );
}

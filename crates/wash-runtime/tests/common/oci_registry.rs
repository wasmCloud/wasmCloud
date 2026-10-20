//! Loopback OCI registry shared by cache and TLS policy tests.
#![allow(dead_code)]

use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_rustls::{TlsAcceptor, rustls::ServerConfig};

pub struct Image {
    manifest: Vec<u8>,
    digest: String,
    config: Vec<u8>,
    config_digest: String,
    pub wasm: Vec<u8>,
    wasm_digest: String,
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

impl Image {
    pub fn new(wasm: &str) -> Self {
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

pub struct Registry {
    pub addr: std::net::SocketAddr,
    pub image: Arc<RwLock<Image>>,
    pub unavailable: Arc<AtomicBool>,
    pub requests: Arc<AtomicUsize>,
    /// The `authorization` header the registry accepts.
    pub authorization: Arc<RwLock<String>>,
    task: tokio::task::JoinHandle<()>,
}

impl Registry {
    pub async fn start() -> Self {
        Self::start_with_tls(None).await
    }

    pub async fn start_with_tls(config: Option<ServerConfig>) -> Self {
        let acceptor = config.map(|config| TlsAcceptor::from(Arc::new(config)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let image = Arc::new(RwLock::new(Image::new("(component)")));
        let unavailable = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let authorization = Arc::new(RwLock::new(String::from("Basic dXNlcjpzZWNyZXQ=")));
        let task = tokio::spawn({
            let image = Arc::clone(&image);
            let requests = Arc::clone(&requests);
            let unavailable = Arc::clone(&unavailable);
            let authorization = Arc::clone(&authorization);
            async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let image = Arc::clone(&image);
                    let requests = Arc::clone(&requests);
                    let unavailable = Arc::clone(&unavailable);
                    let authorization = Arc::clone(&authorization);
                    let acceptor = acceptor.clone();
                    tokio::spawn(async move {
                        match acceptor {
                            Some(acceptor) => {
                                if let Ok(stream) = acceptor.accept(stream).await {
                                    serve(stream, image, requests, unavailable, authorization)
                                        .await;
                                }
                            }
                            None => {
                                serve(stream, image, requests, unavailable, authorization).await
                            }
                        }
                    });
                }
            }
        });
        Self {
            addr,
            image,
            unavailable,
            requests,
            authorization,
            task,
        }
    }

    pub fn reference(&self) -> String {
        format!(
            "localhost:{}/private/component@{}",
            self.addr.port(),
            self.image.read().unwrap().digest
        )
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    stream: impl AsyncRead + AsyncWrite + Unpin,
    image: Arc<RwLock<Image>>,
    requests: Arc<AtomicUsize>,
    unavailable: Arc<AtomicBool>,
    authorization: Arc<RwLock<String>>,
) {
    let service = service_fn(move |req| {
        requests.fetch_add(1, Ordering::SeqCst);
        let response = if unavailable.load(Ordering::SeqCst) {
            Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .body(Full::new(Bytes::new()))
                .unwrap()
        } else {
            answer(req, &image.read().unwrap(), &authorization.read().unwrap())
        };
        std::future::ready(Ok::<_, Infallible>(response))
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .await;
}

fn answer(req: Request<Incoming>, image: &Image, authorization: &str) -> Response<Full<Bytes>> {
    let mut response = Response::builder().header("connection", "close");
    if req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        != Some(authorization)
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

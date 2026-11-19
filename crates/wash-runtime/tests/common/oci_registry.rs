//! Loopback OCI registry shared by cache and TLS policy tests.
#![allow(dead_code)]

use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

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
    /// Further layers after the component, served like it.
    extra_layers: Vec<(String, Vec<u8>)>,
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

impl Image {
    pub fn new(wasm: &str) -> Self {
        Self::build(wat::parse_str(wasm).unwrap(), None, Vec::new())
    }

    /// An image whose component layer is `wasm`, raw bytes not WAT, with its
    /// manifest declaring `declared_size` instead of the real length when
    /// given, followed by `extra_layers` of the same media type.
    pub fn build(wasm: Vec<u8>, declared_size: Option<i64>, extra_layers: Vec<Vec<u8>>) -> Self {
        let wasm_digest = digest(&wasm);
        let extra_layers: Vec<_> = extra_layers
            .into_iter()
            .map(|layer| (digest(&layer), layer))
            .collect();
        let config = serde_json::to_vec(&serde_json::json!({
            "architecture": "wasm", "os": "wasip2", "layerDigests": [&wasm_digest]
        }))
        .unwrap();
        let config_digest = digest(&config);
        let mut layers = vec![serde_json::json!({
            "mediaType": "application/wasm",
            "digest": &wasm_digest,
            "size": declared_size.unwrap_or(wasm.len() as i64)
        })];
        layers.extend(extra_layers.iter().map(|(digest, layer)| {
            serde_json::json!({
                "mediaType": "application/wasm", "digest": digest, "size": layer.len()
            })
        }));
        let manifest = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.wasm.config.v0+json",
                "digest": &config_digest, "size": config.len()
            },
            "layers": layers
        }))
        .unwrap();
        Self {
            digest: digest(&manifest),
            manifest,
            config,
            config_digest,
            wasm,
            wasm_digest,
            extra_layers,
        }
    }

    /// The digest of the component layer.
    pub fn wasm_digest(&self) -> &str {
        &self.wasm_digest
    }

    /// The digests of the layers after the component.
    pub fn extra_layer_digests(&self) -> Vec<String> {
        self.extra_layers.iter().map(|(d, _)| d.clone()).collect()
    }
}

pub struct Registry {
    pub addr: std::net::SocketAddr,
    pub image: Arc<RwLock<Image>>,
    pub unavailable: Arc<AtomicBool>,
    pub requests: Arc<AtomicUsize>,
    /// The path of every request, in arrival order.
    pub paths: Arc<Mutex<Vec<String>>>,
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
        let paths = Arc::new(Mutex::new(Vec::new()));
        let authorization = Arc::new(RwLock::new(String::from("Basic dXNlcjpzZWNyZXQ=")));
        let task = tokio::spawn({
            let image = Arc::clone(&image);
            let requests = Arc::clone(&requests);
            let paths = Arc::clone(&paths);
            let unavailable = Arc::clone(&unavailable);
            let authorization = Arc::clone(&authorization);
            async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let image = Arc::clone(&image);
                    let requests = Arc::clone(&requests);
                    let paths = Arc::clone(&paths);
                    let unavailable = Arc::clone(&unavailable);
                    let authorization = Arc::clone(&authorization);
                    let acceptor = acceptor.clone();
                    tokio::spawn(async move {
                        match acceptor {
                            Some(acceptor) => {
                                if let Ok(stream) = acceptor.accept(stream).await {
                                    serve(
                                        stream,
                                        image,
                                        requests,
                                        paths,
                                        unavailable,
                                        authorization,
                                    )
                                    .await;
                                }
                            }
                            None => {
                                serve(stream, image, requests, paths, unavailable, authorization)
                                    .await
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
            paths,
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
    paths: Arc<Mutex<Vec<String>>>,
    unavailable: Arc<AtomicBool>,
    authorization: Arc<RwLock<String>>,
) {
    let service = service_fn(move |req: Request<Incoming>| {
        requests.fetch_add(1, Ordering::SeqCst);
        paths.lock().unwrap().push(req.uri().path().to_string());
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
    } else if let Some((_, layer)) = image.extra_layers.iter().find(|(d, _)| path.ends_with(d)) {
        layer.clone()
    } else {
        response = response.status(StatusCode::NOT_FOUND);
        Vec::new()
    };
    response
        .header("content-length", body.len())
        .body(Full::new(body.into()))
        .unwrap()
}

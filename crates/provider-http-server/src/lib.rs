//! The httpserver capability provider allows wasmcloud components to receive
//! and process http(s) messages from web browsers, command-line tools
//! such as curl, and other http clients. The server is fully asynchronous,
//! and built on Rust's high-performance axum library, which is in turn based
//! on hyper, and can process a large number of simultaneous connections.
//!
//! ## Features:
//!
//! - HTTP/1 and HTTP/2
//! - TLS
//! - CORS support (select `allowed_origins`, `allowed_methods`,
//!   `allowed_headers`.) Cors has sensible defaults so it should
//!   work as-is for development purposes, and may need refinement
//!   for production if a more secure configuration is required.
//! - All settings can be specified at runtime, using per-component link settings:
//!   - bind path/address
//!   - TLS
//!   - Cors
//! - Flexible confiuration loading: from host, or from local toml or json file.
//! - Fully asynchronous, using tokio lightweight "green" threads
//! - Thread pool (for managing a pool of OS threads). The default
//!   thread pool has one thread per cpu core.
//!

use core::future::Future;
use core::pin::Pin;
use core::str::FromStr as _;
use core::task::{ready, Context, Poll};
use core::time::Duration;

use std::net::{SocketAddr, TcpListener};

use anyhow::{anyhow, bail, Context as _};
use axum::extract;
use bytes::Bytes;
use futures::Stream;
use pin_project_lite::pin_project;
use tokio::task::JoinHandle;
use tokio::{spawn, time};
use tower_http::cors::{self, CorsLayer};
use tracing::{debug, info, trace};
use wasmcloud_provider_sdk::provider::WrpcClient;
use wasmcloud_provider_sdk::{initialize_observability, load_host_data, run_provider};
use wrpc_interface_http::InvokeIncomingHandler as _;

mod address;
mod path;
mod settings;
pub use settings::{default_listen_address, load_settings, ServiceSettings};

pub async fn run() -> anyhow::Result<()> {
    initialize_observability!(
        "http-server-provider",
        std::env::var_os("PROVIDER_HTTP_SERVER_FLAMEGRAPH_PATH")
    );

    let host_data = load_host_data().context("failed to load host data")?;
    match host_data.config.get("routing_mode").map(String::as_str) {
        // Run provider in address mode by default
        Some("address") | None => run_provider(
            address::HttpServerProvider::new(host_data).context(
                "failed to create address-mode HTTP server provider from hostdata configuration",
            )?,
            "http-server-provider",
        )
        .await?
        .await,
        // Run provider in path mode
        Some("path") => {
            run_provider(
                path::HttpServerProvider::new(host_data).await.context(
                    "failed to create path-mode HTTP server provider from hostdata configuration",
                )?,
                "http-server-provider",
            )
            .await?
            .await;
        }
        Some(other) => bail!("unknown routing_mode: {other}"),
    };

    Ok(())
}

/// Build a request to send to the component from the incoming request
pub(crate) fn build_request(
    request: extract::Request,
    scheme: http::uri::Scheme,
    authority: String,
    settings: &ServiceSettings,
) -> Result<http::Request<axum::body::Body>, axum::response::ErrorResponse> {
    let method = request.method();
    if let Some(readonly_mode) = settings.readonly_mode {
        if readonly_mode
            && method != http::method::Method::GET
            && method != http::method::Method::HEAD
        {
            debug!("only GET and HEAD allowed in read-only mode");
            Err((
                http::StatusCode::METHOD_NOT_ALLOWED,
                "only GET and HEAD allowed in read-only mode",
            ))?;
        }
    }
    let (
        http::request::Parts {
            method,
            uri,
            headers,
            ..
        },
        body,
    ) = request.into_parts();
    let http::uri::Parts { path_and_query, .. } = uri.into_parts();

    let mut uri = http::Uri::builder().scheme(scheme);
    if !authority.is_empty() {
        uri = uri.authority(authority);
    }
    if let Some(path_and_query) = path_and_query {
        uri = uri.path_and_query(path_and_query);
    }
    let uri = uri
        .build()
        .map_err(|err| (http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let mut req = http::Request::builder();
    *req.headers_mut().ok_or((
        http::StatusCode::INTERNAL_SERVER_ERROR,
        "invalid request generated",
    ))? = headers;
    let req = req
        .uri(uri)
        .method(method)
        .body(body)
        .map_err(|err| (http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(req)
}

/// Invoke a component with the given request
pub(crate) async fn invoke_component(
    wrpc: &WrpcClient,
    target: &str,
    req: http::Request<axum::body::Body>,
    timeout: Option<Duration>,
    cache_control: Option<&String>,
) -> impl axum::response::IntoResponse {
    // Create a new wRPC client with all headers from the current span injected
    let mut cx = async_nats::HeaderMap::new();
    for (k, v) in
        wasmcloud_provider_sdk::wasmcloud_tracing::context::TraceContextInjector::new_with_extractor(
            &wasmcloud_provider_sdk::wasmcloud_tracing::http::HeaderExtractor(req.headers()),
        )
        .iter()
    {
        cx.insert(k.as_str(), v.as_str());
    }

    trace!(?req, component_id = target, "httpserver calling component");
    let fut = wrpc.invoke_handle_http(Some(cx), req);
    let res = if let Some(timeout) = timeout {
        let Ok(res) = time::timeout(timeout, fut).await else {
            Err(http::StatusCode::REQUEST_TIMEOUT)?
        };
        res
    } else {
        fut.await
    };
    let (res, errors, io) =
        res.map_err(|err| (http::StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}")))?;
    let io = io.map(spawn);
    let errors: Box<dyn Stream<Item = _> + Send + Unpin> = Box::new(errors);
    // TODO: Convert this to http status code
    let mut res =
        res.map_err(|err| (http::StatusCode::INTERNAL_SERVER_ERROR, format!("{err:?}")))?;
    if let Some(cache_control) = cache_control {
        let cache_control = http::HeaderValue::from_str(cache_control)
            .map_err(|err| (http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
        res.headers_mut().append("Cache-Control", cache_control);
    };
    axum::response::Result::<_, axum::response::ErrorResponse>::Ok(res.map(|body| ResponseBody {
        body,
        errors,
        io,
    }))
}

/// Helper function to construct a [`CorsLayer`] according to the [`ServiceSettings`].
pub(crate) fn get_cors_layer(settings: &ServiceSettings) -> anyhow::Result<CorsLayer> {
    let allow_origin = settings.cors_allowed_origins.as_ref();
    let allow_origin: Vec<_> = allow_origin
        .map(|origins| {
            origins
                .iter()
                .map(AsRef::as_ref)
                .map(http::HeaderValue::from_str)
                .collect::<Result<_, _>>()
                .context("failed to parse allowed origins")
        })
        .transpose()?
        .unwrap_or_default();
    let allow_origin = if allow_origin.is_empty() {
        cors::AllowOrigin::any()
    } else {
        cors::AllowOrigin::list(allow_origin)
    };
    let allow_headers = settings.cors_allowed_headers.as_ref();
    let allow_headers: Vec<_> = allow_headers
        .map(|headers| {
            headers
                .iter()
                .map(AsRef::as_ref)
                .map(http::HeaderName::from_str)
                .collect::<Result<_, _>>()
                .context("failed to parse allowed header names")
        })
        .transpose()?
        .unwrap_or_default();
    let allow_headers = if allow_headers.is_empty() {
        cors::AllowHeaders::any()
    } else {
        cors::AllowHeaders::list(allow_headers)
    };
    let allow_methods = settings.cors_allowed_methods.as_ref();
    let allow_methods: Vec<_> = allow_methods
        .map(|methods| {
            methods
                .iter()
                .map(AsRef::as_ref)
                .map(http::Method::from_str)
                .collect::<Result<_, _>>()
                .context("failed to parse allowed methods")
        })
        .transpose()?
        .unwrap_or_default();
    let allow_methods = if allow_methods.is_empty() {
        cors::AllowMethods::any()
    } else {
        cors::AllowMethods::list(allow_methods)
    };
    let expose_headers = settings.cors_exposed_headers.as_ref();
    let expose_headers: Vec<_> = expose_headers
        .map(|headers| {
            headers
                .iter()
                .map(AsRef::as_ref)
                .map(http::HeaderName::from_str)
                .collect::<Result<_, _>>()
                .context("failed to parse exposeed header names")
        })
        .transpose()?
        .unwrap_or_default();
    let expose_headers = if expose_headers.is_empty() {
        cors::ExposeHeaders::any()
    } else {
        cors::ExposeHeaders::list(expose_headers)
    };
    let mut cors = CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_headers(allow_headers)
        .allow_methods(allow_methods)
        .expose_headers(expose_headers);
    if let Some(max_age) = settings.cors_max_age_secs {
        cors = cors.max_age(Duration::from_secs(max_age));
    }

    Ok(cors)
}

/// Helper function to create and listen on a [`TcpListener`] from the given [`ServiceSettings`].
///
/// Note that this function actually calls the `bind` method on the [`TcpSocket`], it's up to the
/// caller to ensure that the address is not already in use (or to handle the error if it is).
pub(crate) fn get_tcp_listener(settings: &ServiceSettings) -> anyhow::Result<TcpListener> {
    let socket = match &settings.address {
        SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4(),
        SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6(),
    }
    .context("Unable to open socket")?;
    // Copied this option from
    // https://github.com/bytecodealliance/wasmtime/blob/05095c18680927ce0cf6c7b468f9569ec4d11bd7/src/commands/serve.rs#L319.
    // This does increase throughput by 10-15% which is why we're creating the socket. We're
    // using the tokio one because it exposes the `reuseaddr` option.
    socket
        .set_reuseaddr(!cfg!(windows))
        .context("Error when setting socket to reuseaddr")?;
    socket
        .set_nodelay(true)
        .context("failed to set `TCP_NODELAY`")?;

    match settings.disable_keepalive {
        Some(false) => {
            info!("disabling TCP keepalive");
            socket
                .set_keepalive(false)
                .context("failed to disable TCP keepalive")?
        }
        None | Some(true) => socket
            .set_keepalive(true)
            .context("failed to enable TCP keepalive")?,
    }

    socket
        .bind(settings.address)
        .context("Unable to bind to address")?;
    let listener = socket.listen(1024).context("unable to listen on socket")?;
    let listener = listener.into_std().context("Unable to get listener")?;

    Ok(listener)
}

pin_project! {
    struct ResponseBody {
        #[pin]
        body: wrpc_interface_http::HttpBody,
        #[pin]
        errors: Box<dyn Stream<Item = wrpc_interface_http::HttpBodyError<axum::Error>> + Send + Unpin>,
        #[pin]
        io: Option<JoinHandle<anyhow::Result<()>>>,
    }
}

impl http_body::Body for ResponseBody {
    type Data = Bytes;
    type Error = anyhow::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let mut this = self.as_mut().project();
        if let Some(io) = this.io.as_mut().as_pin_mut() {
            match io.poll(cx) {
                Poll::Ready(Ok(Ok(()))) => {
                    this.io.take();
                }
                Poll::Ready(Ok(Err(err))) => {
                    return Poll::Ready(Some(Err(
                        anyhow!(err).context("failed to complete async I/O")
                    )))
                }
                Poll::Ready(Err(err)) => {
                    return Poll::Ready(Some(Err(anyhow!(err).context("I/O task failed"))))
                }
                Poll::Pending => {}
            }
        }
        match this.errors.poll_next(cx) {
            Poll::Ready(Some(err)) => {
                if let Some(io) = this.io.as_pin_mut() {
                    io.abort();
                }
                return Poll::Ready(Some(Err(anyhow!(err).context("failed to process body"))));
            }
            Poll::Ready(None) | Poll::Pending => {}
        }
        match ready!(this.body.poll_frame(cx)) {
            Some(Ok(frame)) => Poll::Ready(Some(Ok(frame))),
            Some(Err(err)) => {
                if let Some(io) = this.io.as_pin_mut() {
                    io.abort();
                }
                Poll::Ready(Some(Err(err)))
            }
            None => {
                if let Some(io) = this.io.as_pin_mut() {
                    io.abort();
                }
                Poll::Ready(None)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use anyhow::Result;
    use futures::StreamExt;
    use wasmcloud_provider_sdk::{
        provider::initialize_host_data, run_provider, HostData, InterfaceLinkDefinition,
    };
    use wasmcloud_test_util::testcontainers::{AsyncRunner, NatsServer};

    use crate::{address, path};

    // This test is ignored by default as it requires a container runtime to be installed
    // to run the testcontainer. In GitHub Actions CI, this is only works on `linux`
    #[ignore]
    #[tokio::test]
    async fn can_listen_and_invoke_with_timeout() -> Result<()> {
        let nats_container = NatsServer::default()
            .start()
            .await
            .expect("failed to start nats-server container");
        let nats_port = nats_container
            .get_host_port_ipv4(4222)
            .await
            .expect("should be able to find the NATS port");
        let nats_address = format!("nats://127.0.0.1:{nats_port}");

        let default_address = "0.0.0.0:8080";
        let host_data = HostData {
            lattice_rpc_url: nats_address.clone(),
            lattice_rpc_prefix: "lattice".to_string(),
            provider_key: "http-server-provider-test".to_string(),
            config: std::collections::HashMap::from([
                ("default_address".to_string(), default_address.to_string()),
                ("routing_mode".to_string(), "address".to_string()),
            ]),
            link_definitions: vec![InterfaceLinkDefinition {
                source_id: "http-server-provider-test".to_string(),
                target: "test-component".to_string(),
                name: "default".to_string(),
                wit_namespace: "wasi".to_string(),
                wit_package: "http".to_string(),
                interfaces: vec!["incoming-handler".to_string()],
                source_config: std::collections::HashMap::from([(
                    "timeout_ms".to_string(),
                    "100".to_string(),
                )]),
                target_config: HashMap::new(),
                source_secrets: None,
                target_secrets: None,
            }],
            ..Default::default()
        };
        initialize_host_data(host_data.clone()).expect("should be able to initialize host data");

        let provider = run_provider(
            address::HttpServerProvider::new(&host_data)
                .expect("should be able to create provider"),
            "http-server-provider-test",
        )
        .await
        .expect("should be able to run provider");

        // Use a separate task to listen for the component message
        let conn = async_nats::connect(nats_address)
            .await
            .expect("should be able to connect");
        let mut subscriber = conn
            .subscribe("lattice.test-component.wrpc.>")
            .await
            .expect("should be able to subscribe");

        let provider_handle = tokio::spawn(provider);

        // Let the provider have a second to setup the listener
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        let resp = reqwest::get("http://127.0.0.1:8080")
            .await
            .expect("should be able to make request");

        // Should have timed out
        assert_eq!(resp.status(), 408);
        // Ensure component received the message
        let msg = subscriber
            .next()
            .await
            .expect("should be able to get a message");
        assert!(msg.subject.contains("test-component"));
        provider_handle.abort();
        let _ = nats_container.stop().await;

        Ok(())
    }

    // This test is ignored by default as it requires a container runtime to be installed
    // to run the testcontainer. In GitHub Actions CI, this is only works on `linux`
    #[ignore]
    #[tokio::test]
    async fn can_support_path_based_routing() -> Result<()> {
        let nats_container = NatsServer::default()
            .start()
            .await
            .expect("failed to start nats-server container");
        let nats_port = nats_container
            .get_host_port_ipv4(4222)
            .await
            .expect("should be able to find the NATS port");
        let nats_address = format!("nats://127.0.0.1:{nats_port}");

        let default_address = "0.0.0.0:8081";
        let host_data = HostData {
            lattice_rpc_url: nats_address.clone(),
            lattice_rpc_prefix: "lattice".to_string(),
            provider_key: "http-server-provider-test".to_string(),
            config: std::collections::HashMap::from([
                ("default_address".to_string(), default_address.to_string()),
                ("routing_mode".to_string(), "path".to_string()),
                ("timeout_ms".to_string(), "100".to_string()),
            ]),
            link_definitions: vec![
                InterfaceLinkDefinition {
                    source_id: "http-server-provider-test".to_string(),
                    target: "test-component-one".to_string(),
                    name: "default".to_string(),
                    wit_namespace: "wasi".to_string(),
                    wit_package: "http".to_string(),
                    interfaces: vec!["incoming-handler".to_string()],
                    source_config: std::collections::HashMap::from([(
                        "path".to_string(),
                        "/foo".to_string(),
                    )]),
                    target_config: HashMap::new(),
                    source_secrets: None,
                    target_secrets: None,
                },
                InterfaceLinkDefinition {
                    source_id: "http-server-provider-test".to_string(),
                    target: "test-component-two".to_string(),
                    name: "default".to_string(),
                    wit_namespace: "wasi".to_string(),
                    wit_package: "http".to_string(),
                    interfaces: vec!["incoming-handler".to_string()],
                    source_config: std::collections::HashMap::from([(
                        "path".to_string(),
                        "/bar".to_string(),
                    )]),
                    target_config: HashMap::new(),
                    source_secrets: None,
                    target_secrets: None,
                },
            ],
            ..Default::default()
        };
        initialize_host_data(host_data.clone()).expect("should be able to initialize host data");

        let provider = run_provider(
            path::HttpServerProvider::new(&host_data)
                .await
                .expect("should be able to create provider"),
            "http-server-provider-test",
        )
        .await
        .expect("should be able to run provider");

        // Use a separate task to listen for the component message
        let conn = async_nats::connect(nats_address)
            .await
            .expect("should be able to connect");
        let mut subscriber_one = conn
            .subscribe("lattice.test-component-one.wrpc.>")
            .await
            .expect("should be able to subscribe");
        let mut subscriber_two = conn
            .subscribe("lattice.test-component-two.wrpc.>")
            .await
            .expect("should be able to subscribe");

        let provider_handle = tokio::spawn(provider);
        // Let the provider have a second to setup the listeners
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        // Invoke component one
        let resp = reqwest::get("http://127.0.0.1:8081/foo")
            .await
            .expect("should be able to make request");
        // Should have timed out
        assert_eq!(resp.status(), 408);
        let msg = subscriber_one
            .next()
            .await
            .expect("should be able to get a message");
        assert!(msg.subject.contains("test-component-one"));

        // Invoke component two
        let resp = reqwest::get("http://127.0.0.1:8081/bar")
            .await
            .expect("should be able to make request");
        // Should have timed out
        assert_eq!(resp.status(), 408);
        let msg = subscriber_two
            .next()
            .await
            .expect("should be able to get a message");
        assert!(msg.subject.contains("test-component-two"));

        // Invoke component two with a query parameter
        let resp = reqwest::get("http://127.0.0.1:8081/bar?someparam=foo")
            .await
            .expect("should be able to make request");
        // Should have timed out
        assert_eq!(resp.status(), 408);
        let msg = subscriber_two
            .next()
            .await
            .expect("should be able to get a message");
        assert!(msg.subject.contains("test-component-two"));

        // Unknown path should return 404
        let resp = reqwest::get("http://127.0.0.1:8081/some/other/route/idk")
            .await
            .expect("should be able to make request");
        assert_eq!(resp.status(), 404);

        // No other messages should have been received
        // (the assertion is that the operation timed out)
        assert!(
            tokio::time::timeout(tokio::time::Duration::from_secs(1), subscriber_one.next())
                .await
                .is_err(),
        );
        assert!(
            tokio::time::timeout(tokio::time::Duration::from_secs(1), subscriber_two.next())
                .await
                .is_err(),
        );

        provider_handle.abort();
        let _ = nats_container.stop().await;

        Ok(())
    }
}

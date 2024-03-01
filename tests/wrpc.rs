use core::str::FromStr as _;
use core::time::Duration;

use std::net::Ipv6Addr;
use std::sync::Arc;

use anyhow::{anyhow, ensure, Context as _};
use futures::stream;
use futures::{StreamExt as _, TryStreamExt as _};
use hyper::header::HOST;
use hyper::Uri;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use test_actors::{RUST_WRPC_PINGER_COMPONENT, RUST_WRPC_PONGER_COMPONENT};
use tokio::net::TcpListener;
use tokio::try_join;
use tracing::info;
use tracing_subscriber::prelude::*;
use uuid::Uuid;
use wasmcloud_test_util::{
    actor::assert_scale_actor, host::WasmCloudTestHost, lattice::link::assert_advertise_link,
};
use wrpc_transport::{AcceptedInvocation, Transmitter as _};

pub mod common;
use common::nats::start_nats;

const LATTICE: &str = "default";
const PINGER_COMPONENT_ID: &str = "wrpc_pinger_component";
const PONGER_COMPONENT_ID: &str = "wrpc_ponger_component";

#[tokio::test]
async fn wrpc() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    // Start NATS server
    let (nats_server, nats_url, nats_client) =
        start_nats().await.expect("should be able to start NATS");
    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();
    let wrpc_client = wrpc_transport_nats::Client::new(
        nats_client.clone(),
        format!("{LATTICE}.{PINGER_COMPONENT_ID}"),
    );
    let wrpc_client = Arc::new(wrpc_client);
    // Build the host
    let host = WasmCloudTestHost::start(&nats_url, LATTICE, None, None)
        .await
        .context("failed to start test host")?;

    let mut outgoing_http_invocations =
        wrpc_interface_http::OutgoingHandler::serve_handle(wrpc_client.as_ref())
            .await
            .context("failed to serve `wrpc:http/outgoing-handler` invocations")?;

    // Scale pinger
    assert_scale_actor(
        &ctl_client,
        &host.host_key(),
        format!("file://{RUST_WRPC_PINGER_COMPONENT}"),
        PINGER_COMPONENT_ID,
        None,
        5,
    )
    .await
    .expect("should've scaled pinger actor");

    // Scale ponger
    assert_scale_actor(
        &ctl_client,
        &host.host_key(),
        format!("file://{RUST_WRPC_PONGER_COMPONENT}"),
        PONGER_COMPONENT_ID,
        None,
        5,
    )
    .await
    .expect("should've scaled actor");

    // Link pinger --wrpc:testing/pingpong--> ponger
    assert_advertise_link(
        &ctl_client,
        PINGER_COMPONENT_ID,
        PONGER_COMPONENT_ID,
        "default",
        "test-actors",
        "testing",
        vec!["pingpong".to_string(), "busybox".to_string()],
        vec![],
        vec![],
    )
    .await
    .expect("should advertise link");

    // Link pinger --wasi:http/outgoing-handler--> pinger
    assert_advertise_link(
        &ctl_client,
        PINGER_COMPONENT_ID,
        PINGER_COMPONENT_ID,
        "default",
        "wasi",
        "http",
        vec!["outgoing-handler".to_string()],
        vec![],
        vec![],
    )
    .await
    .expect("should advertise link");

    try_join!(
        async {
            let listener = TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
                .await
                .context("failed to start TCP listener")?;
            let addr = listener
                .local_addr()
                .context("failed to query listener local address")?;
            try_join!(
                async {
                    info!("await connection");
                    let (stream, addr) = listener
                        .accept()
                        .await
                        .context("failed to accept connection")?;
                    info!("accepted connection from {addr}");
                    let wrpc_client = Arc::clone(&wrpc_client);
                    hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                         .serve_connection(
                             TokioIo::new(stream),
                             hyper::service::service_fn(
                                 move |mut request: hyper::Request<hyper::body::Incoming>| {
                                     use wrpc_interface_http::IncomingHandler as _;

                                     let host = request.headers().get(HOST).expect("`host` header missing");
                                     let host = host.to_str().expect("`host` header value is not a valid string");
                                     let path_and_query = request.uri().path_and_query().expect("`path_and_query` missing");
                                     let uri = Uri::builder()
                                         .scheme("http")
                                         .authority(host)
                                         .path_and_query(path_and_query.clone())
                                         .build()
                                         .expect("failed to build request URI");
                                     *request.uri_mut() = uri;
                                     let wrpc_client = Arc::clone(&wrpc_client);
                                     async move {
                                         info!(?request, "invoke `handle`");
                                         let (response, tx, errors) =
                                             wrpc_client.invoke_handle_hyper(request).await.context(
                                                 "failed to invoke `wrpc:http/incoming-handler.handle`",
                                             )?;
                                         info!("await parameter transmit");
                                         tx.await.context("failed to transmit parameters")?;
                                         info!("await error collect");
                                         let errors: Vec<_> = errors.collect().await;
                                         assert!(errors.is_empty());
                                         info!("request served");
                                         response
                                     }
                                 },
                             ),
                         )
                         .await
                         .map_err(|err| anyhow!(err).context("failed to serve connection"))
                },
                async {
                    let http_client = reqwest::Client::builder()
                        .timeout(Duration::from_secs(20))
                        .connect_timeout(Duration::from_secs(20))
                        .build()
                        .context("failed to build HTTP client")?;
                    let http_res = http_client
                        .post(format!("http://localhost:{}/foo?bar=baz", addr.port()))
                        .header("test-header", "test-value")
                        .body(
                             format!(r#"{{"min":42,"max":4242,"port":4242,"config_key":"test-config-data","authority":"localhost:{}"}}"#,
                                addr.port())
                         )
                        .send()
                        .await
                        .context("failed to connect to server")?
                        .text()
                        .await
                        .context("failed to get response text")?;
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    // NOTE: If values are truly random, we have nothing to assert for some of these fields
                    struct Response {
                        #[allow(dead_code)]
                        get_random_bytes: [u8; 8],
                        #[allow(dead_code)]
                        get_random_u64: u64,
                        guid: String,
                        random_in_range: u32,
                        #[allow(dead_code)]
                        random_32: u32,
                        #[allow(dead_code)]
                        long_value: String,
                        config_value: Option<Vec<u8>>,
                        all_config: Vec<(String, Vec<u8>)>,
                        ping: String,
                        meaning_of_universe: u8,
                        split: Vec<String>,
                        is_same: bool,
                        archie: bool,
                    }
                    let Response {
                        get_random_bytes: _,
                        get_random_u64: _,
                        guid,
                        random_32: _,
                        random_in_range,
                        long_value,
                        config_value,
                        all_config,
                        ping,
                        meaning_of_universe,
                        split,
                        is_same,
                        archie,
                    } = serde_json::from_str(&http_res).context("failed to decode body as JSON")?;
                    ensure!(Uuid::from_str(&guid).is_ok());
                    ensure!(
                        (42..=4242).contains(&random_in_range),
                        "{random_in_range} should have been within range from 42 to 4242 inclusive"
                    );
                    ensure!(config_value.is_none());
                    ensure!(all_config == []);
                    ensure!(ping == "pong");
                    ensure!(long_value == "1234567890".repeat(1000));
                    ensure!(meaning_of_universe == 42);
                    ensure!(split == ["hi", "there", "friend"]);
                    ensure!(is_same);
                    ensure!(archie);
                    Ok(())
                }
            )?;
            anyhow::Ok(())
        },
        async {
            let AcceptedInvocation {
                params:
                    (
                        wrpc_interface_http::Request {
                            mut body,
                            trailers,
                            method,
                            path_with_query,
                            scheme,
                            authority,
                            headers,
                        },
                        opts,
                    ),
                result_subject,
                transmitter,
                ..
            } = outgoing_http_invocations
                .try_next()
                .await
                .context("failed to accept `wrpc:http/outgoing-handler.handle` invocation")?
                .context("invocation stream unexpectedly finished")?;
            assert_eq!(method, wrpc_interface_http::Method::Put);
            assert_eq!(path_with_query.as_deref(), Some("/test"));
            assert_eq!(scheme, Some(wrpc_interface_http::Scheme::HTTPS));
            assert_eq!(authority.as_deref(), Some("localhost:4242"));
            assert_eq!(
                headers,
                vec![("host".into(), vec!["localhost:4242".into()])],
            );
            // wasmtime defaults
            assert_eq!(
                opts,
                Some(wrpc_interface_http::RequestOptions {
                    connect_timeout: Some(Duration::from_secs(600)),
                    first_byte_timeout: Some(Duration::from_secs(600)),
                    between_bytes_timeout: Some(Duration::from_secs(600)),
                })
            );
            try_join!(
                async {
                    info!("transmit response");
                    transmitter
                        .transmit_static(
                            result_subject,
                            Ok::<_, wrpc_interface_http::ErrorCode>(
                                wrpc_interface_http::Response {
                                    body: stream::iter([("test".into())]),
                                    trailers: async { None },
                                    status: 200,
                                    headers: Vec::default(),
                                },
                            ),
                        )
                        .await
                        .context("failed to transmit response")?;
                    info!("response transmitted");
                    anyhow::Ok(())
                },
                async {
                    info!("await request body element");
                    let item = body
                        .try_next()
                        .await
                        .context("failed to receive body item")?
                        .context("unexpected end of body stream")?;
                    assert_eq!(String::from_utf8(item).unwrap(), "test");
                    info!("await request body end");
                    let item = body
                        .try_next()
                        .await
                        .context("failed to receive end item")?;
                    assert_eq!(item, None);
                    info!("request body verified");
                    Ok(())
                },
                async {
                    info!("await request trailers");
                    let trailers = trailers.await.context("failed to receive trailers")?;
                    assert_eq!(trailers, None);
                    info!("request trailers verified");
                    Ok(())
                }
            )?;
            Ok(())
        }
    )?;
    nats_server
        .stop()
        .await
        .expect("should be able to stop NATS");
    Ok(())
}

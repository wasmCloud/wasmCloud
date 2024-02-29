use core::time::Duration;

use anyhow::Context;
use futures::stream;
use futures::TryStreamExt as _;
use test_actors::{RUST_WRPC_PINGER_COMPONENT, RUST_WRPC_PONGER_COMPONENT};
use tokio::try_join;
use tracing::info;
use tracing_subscriber::prelude::*;
use wasmcloud_test_util::{
    actor::assert_scale_actor, host::WasmCloudTestHost, lattice::link::assert_advertise_link,
};
use wrpc_transport::{AcceptedInvocation, Client as _, Transmitter as _};

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
    // Build the host
    let host = WasmCloudTestHost::start(&nats_url, LATTICE, None, None)
        .await
        .context("failed to start test host")?;

    let mut outgoing_http_invocations =
        wrpc_interface_http::OutgoingHandler::serve_handle(&wrpc_client)
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
        "wasmcloud",
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
            let (results, tx) = wrpc_client
                .invoke_static::<String>("wasmcloud:testing/invoke", "call", ())
                .await
                .context("invocation failed")?;
            tx.await.context("failed to transmit parameters")?;
            assert_eq!(
                results,
                r#"Ping pong, meaning of universe is: 42, split: ["hi", "there", "friend"], is_same: true, archie good boy: true"#
            );
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

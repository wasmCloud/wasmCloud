use std::collections::HashMap;
use std::net::{Ipv6Addr, TcpListener};
use std::path::Path;

use anyhow::Context;
use tracing_subscriber::prelude::*;
use wasmcloud_host::local::{ActorConfig, Lattice, LatticeConfig, LinkConfig, TcpSocketConfig};
use wasmcloud_host::url::Url;

fn wasm_url(path: impl AsRef<Path>) -> Url {
    Url::from_file_path(path).expect("failed to parse Wasm path")
}

#[tokio::test(flavor = "multi_thread")]
async fn local() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    let port = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0))
        .context("failed to start TCP listener")?
        .local_addr()
        .context("failed to query listener local address")?
        .port();

    let _lattice = Lattice::new(LatticeConfig {
        actors: HashMap::from([
            (
                "logging".into(),
                ActorConfig {
                    url: wasm_url(test_actors::RUST_LOGGING_MODULE_COMMAND),
                },
            ),
            (
                "http".into(),
                ActorConfig {
                    url: wasm_url(test_actors::RUST_HTTP_COMPAT_COMMAND_PREVIEW2),
                },
            ),
            (
                "tcp".into(),
                ActorConfig {
                    url: wasm_url(test_actors::RUST_TCP_COMPONENT_COMMAND_PREVIEW2),
                },
            ),
        ]),
        links: vec![
            LinkConfig::Interface {
                name: "default:http-server/HttpServer.HandleRequest".into(),
                source: "logging".into(),
                target: "http".into(),
            },
            LinkConfig::Interface {
                name: "wasi:logging/logging".into(),
                source: "tcp".into(),
                target: "logging".into(),
            },
            LinkConfig::Tcp {
                socket: TcpSocketConfig {
                    addr: format!("[::]:{port}"),
                },
                chain: vec!["tcp".into()],
            },
        ],
    })
    .await
    .context("failed to initialize cloud")?;

    eprintln!("sending a GET request on port `{port}`");

    let res = reqwest::get(format!("http://localhost:{port}"))
        .await?
        .text()
        .await?;
    assert_eq!(
        res,
        "[tcp-component-command] received an HTTP GET request with body: ``"
    );

    let res = reqwest::Client::new()
        .post(format!("http://localhost:{port}"))
        .body("42")
        .send()
        .await?
        .text()
        .await?;
    assert_eq!(
        res,
        "[tcp-component-command] received an HTTP POST request with body: `42`"
    );
    Ok(())
}

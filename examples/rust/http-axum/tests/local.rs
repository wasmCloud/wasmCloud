#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::net::{Ipv6Addr, TcpListener};

use anyhow::Context;
use tokio::process::Command;
use tracing_subscriber::prelude::*;
use wasmcloud_host::local::{ActorConfig, Lattice, LatticeConfig, LinkConfig, TcpSocketConfig};
use wasmcloud_host::url::Url;

#[tokio::test(flavor = "multi_thread")]
async fn local() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn")
            }),
        )
        .init();

    let port = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0))
        .context("failed to start TCP listener")?
        .local_addr()
        .context("failed to query listener local address")?
        .port();

    Command::new(env!("CARGO"))
        .args([
            "build",
            "--target",
            "wasm32-wasi",
            "--target-dir",
            env!("CARGO_TARGET_TMPDIR"),
        ])
        .status()
        .await
        .context("failed to build actor")?
        .success()
        .then_some(())
        .context("actor build failed")?;
    let url = Url::from_file_path(format!(
        "{}/wasm32-wasi/debug/http-axum.wasm",
        env!("CARGO_TARGET_TMPDIR")
    ))
    .expect("failed to parse Wasm path");
    let _lattice = Lattice::new(LatticeConfig {
        actors: HashMap::from([("server".into(), ActorConfig { url })]),
        links: vec![LinkConfig::Tcp {
            socket: TcpSocketConfig {
                addr: format!("[::]:{port}"),
            },
            chain: vec!["server".into()],
        }],
    })
    .await
    .context("failed to initialize lattice")?;

    eprintln!("sending a GET request on port `{port}`");

    let res = reqwest::get(format!("http://localhost:{port}"))
        .await?
        .text()
        .await?;
    assert_eq!(res, "Hello, World!");
    Ok(())
}

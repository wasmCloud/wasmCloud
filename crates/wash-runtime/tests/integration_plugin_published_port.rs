//! A host component plugin listens on a real port.
//!
//! The `socket-echo-plugin` fixture binds `127.0.0.1:50051` inside its own
//! private virtual loopback from `wasi:cli/run` and echoes. Nothing can reach
//! that listener on its own — the host publishes a real port and splices
//! accepted connections into it.
//!
//! What these tests pin, beyond "bytes move":
//!   - publishing is refused unless the host opted in, rather than silently
//!     leaving a declared port unexposed
//!   - a connection arriving before the plugin's accept loop is up is held
//!     through the readiness window instead of being reset
//!   - two plugins claiming one real port is a start failure naming both
//!
//! That the guest is handed the *real* external peer address is asserted in
//! `host::ports`'s own tests, where the accepted connection is inspectable —
//! the echo fixture has no way to report what it saw.

#![cfg(feature = "host-component-plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use wash_runtime::host::HostApi;
use wash_runtime::host::declared_port::{DeclaredPort, Protocol};
use wash_runtime::host::ports::{PortTable, PublishConfig};

mod common;
use common::start_bare_host_with_plugin_ports;

const SOCKET_ECHO_PLUGIN_WASM: &[u8] = include_bytes!("wasm/socket_echo_plugin.wasm");

/// The port the fixture binds inside its own virtual network.
const ECHO_VIRTUAL_PORT: u16 = 50051;

/// Reserve a free TCP port by binding and releasing it, so a test can declare a
/// concrete `publish` number without colliding with a parallel test.
async fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn published(port: u16) -> DeclaredPort {
    DeclaredPort {
        name: "echo".into(),
        port: ECHO_VIRTUAL_PORT,
        protocol: Protocol::Tcp,
        publish: Some(port),
        bind: None,
    }
}

/// Build a host running the echo plugin with `ports` declared, under `config`.
async fn start_host_with_echo_plugin(
    ports: Vec<DeclaredPort>,
    config: PublishConfig,
    table: Arc<PortTable>,
) -> Result<impl HostApi> {
    start_bare_host_with_plugin_ports(
        "socket-echo-plugin",
        SOCKET_ECHO_PLUGIN_WASM,
        ports,
        config,
        table,
    )
    .await
}

fn enabled_config() -> PublishConfig {
    PublishConfig {
        enabled: true,
        readiness_timeout: Duration::from_secs(10),
        ..Default::default()
    }
}

/// Round-trip real bytes, external client through the splice to a wasm plugin's
/// virtual listener and back.
#[tokio::test(flavor = "multi_thread")]
async fn a_published_plugin_port_echoes_for_an_external_client() -> Result<()> {
    let port = free_port().await?;
    let _host =
        start_host_with_echo_plugin(vec![published(port)], enabled_config(), PortTable::new())
            .await?;

    let real_addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let mut client = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(real_addr))
        .await
        .context("connecting to the published port timed out")??;

    client.write_all(b"hello splice").await?;
    let mut buf = vec![0u8; b"hello splice".len()];
    tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut buf))
        .await
        .context("waiting for the plugin's echo timed out")??;
    assert_eq!(&buf, b"hello splice");

    Ok(())
}

/// The plugin's listener is registered from its run loop, so it does not exist
/// when `start()` returns. A client connecting immediately must be held, not
/// reset — that interval reopens on every supervised restart, so getting it
/// wrong shows up as flaky first requests after every deploy.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_arriving_before_the_accept_loop_is_held_not_reset() -> Result<()> {
    let port = free_port().await?;
    let table = PortTable::new();
    let host = start_host_with_echo_plugin(vec![published(port)], enabled_config(), table).await;

    // Connect as early as possible — ideally before the plugin's `cli/run` has
    // reached its `bind`.
    let real_addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let mut client = TcpStream::connect(real_addr)
        .await
        .context("the real port should accept before the guest listener exists")?;
    let _host = host?;

    client.write_all(b"early").await?;
    let mut buf = vec![0u8; 5];
    tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut buf))
        .await
        .context("a connection held through the readiness window should still be served")??;
    assert_eq!(&buf, b"early");

    Ok(())
}

/// Declaring a published port on a host that did not opt in must fail loudly.
/// Coming up with the port unexposed would look like success to an operator
/// whose manifest says otherwise.
#[tokio::test(flavor = "multi_thread")]
async fn publishing_without_the_host_flag_fails_the_plugin_start() {
    let port = free_port().await.unwrap();
    let err = start_host_with_echo_plugin(
        vec![published(port)],
        PublishConfig::default(),
        PortTable::new(),
    )
    .await
    .map(|_| ())
    .expect_err("a declared published port must not start on a host that forbids publishing");
    let err = format!("{err:#}");
    assert!(err.contains("--publish-ports"), "got: {err}");
}

/// A port declared without `publish` binds inside the plugin and nothing else:
/// no reservation, no listener, and no error either.
#[tokio::test(flavor = "multi_thread")]
async fn a_declared_but_unpublished_port_exposes_nothing() -> Result<()> {
    let table = PortTable::new();
    let declared = DeclaredPort {
        name: "echo".into(),
        port: ECHO_VIRTUAL_PORT,
        protocol: Protocol::Tcp,
        publish: None,
        bind: None,
    };
    // Publishing is disabled, and this must still start: nothing is exposed.
    let _host =
        start_host_with_echo_plugin(vec![declared], PublishConfig::default(), Arc::clone(&table))
            .await?;

    assert!(
        !table.is_published(
            Protocol::Tcp,
            format!("127.0.0.1:{ECHO_VIRTUAL_PORT}").parse()?
        ),
        "an unpublished port must not reserve anything"
    );
    Ok(())
}

/// Two plugins on one host cannot both claim a real port. The point of a single
/// host-owned table is that this is a start failure naming the other holder,
/// rather than two listeners that each believe they own the address.
#[tokio::test(flavor = "multi_thread")]
async fn a_port_conflict_between_plugins_names_the_other_holder() -> Result<()> {
    let port = free_port().await?;
    let table = PortTable::new();

    let _first =
        start_host_with_echo_plugin(vec![published(port)], enabled_config(), Arc::clone(&table))
            .await?;

    let err = start_host_with_echo_plugin(vec![published(port)], enabled_config(), table)
        .await
        .map(|_| ())
        .expect_err("the second claim on a published port must fail");
    let err = format!("{err:#}");
    assert!(
        err.contains("already published by") && err.contains("socket-echo-plugin"),
        "the conflict should name the holder, got: {err}"
    );
    Ok(())
}

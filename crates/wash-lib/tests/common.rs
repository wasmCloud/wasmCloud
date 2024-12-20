use std::net::{Ipv4Addr, SocketAddrV4};

use anyhow::{Context as _, Result};
use tokio::net::TcpListener;

pub const NATS_SERVER_VERSION: &str = "v2.10.20";

/// Returns an open port on the interface, searching within the range endpoints, inclusive
pub async fn find_open_port() -> Result<u16> {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .context("failed to bind random port")?
        .local_addr()
        .map(|addr| addr.port())
        .context("failed to get local address from opened TCP socket")
}

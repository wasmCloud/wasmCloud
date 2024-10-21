use std::net::{Ipv4Addr, SocketAddrV4};

use anyhow::{Context as _, Result};
use tokio::net::TcpListener;

/// Returns an open IPv4 port
pub async fn free_port_ipv4() -> Result<u16> {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .context("failed to bind random port")?
        .local_addr()
        .map(|addr| addr.port())
        .context("failed to get local address from opened TCP socket")
}

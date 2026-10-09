//! Guest-level tests for a raw socket permitted by an `allowed_hosts` entry
//! that names a host.
//!
//! A real guest resolves a name through the system resolver and then opens a
//! `wasi:sockets` TCP connection to the address it was handed, on both the
//! p2 path (the lookup stream and `std`'s sockets) and the p3 path. Uses the
//! http-ip-name-lookup fixtures: the request path is the name to resolve
//! (`/-` resolves nothing), `?connect=<ip>:<port>` is where to connect, and
//! the body reports `connect: denied` when the host's socket policy refused
//! it, or `connect: failed: …` when the network answered instead.
//!
//! The destination is a closed port on this machine's own address, so a
//! permitted connect fails at the network — refused, usually — and a refused
//! one never leaves the host. No connection is ever accepted, which keeps
//! the tests clear of local firewalls.
//!
//! The name has to resolve, through the system resolver, to an address that
//! is not loopback: a loopback address is the guest's virtual network and
//! never reaches the egress policy. This machine's own hostname is tried
//! first. Where that does not resolve, a wildcard DNS name that answers with
//! this machine's own address (`<ip>.sslip.io`, `<ip>.nip.io`) is used, which
//! needs working DNS. Where none of them resolve the tests say so and pass
//! without running.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, ToSocketAddrs},
    time::Duration,
};
use tokio::time::timeout;

use common::{http_only_host_interfaces, start_host_with_p3_http_handler};
use wash_runtime::{
    host::HostApi,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

const PREVIEWS: &[(&[u8], &str)] = &[
    (include_bytes!("wasm/http_ip_name_lookup.wasm"), "p2"),
    (include_bytes!("wasm/http_ip_name_lookup_p3.wasm"), "p3"),
];

/// A name the system resolver answers with a real address, and a closed port
/// on that address.
struct Destination {
    name: String,
    addr: Ipv4Addr,
    port: u16,
}

impl Destination {
    /// A name the resolver answers with an address the egress policy treats
    /// as real egress: this machine's hostname, else a wildcard DNS name for
    /// its own address.
    fn discover() -> Option<Self> {
        let hostname = std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|hostname| hostname.trim().to_string())
            .filter(|hostname| !hostname.is_empty());
        let own_address = own_address();
        let candidates = hostname
            .iter()
            .flat_map(|hostname| [hostname.clone(), format!("{hostname}.local")])
            .chain(
                own_address
                    .iter()
                    .flat_map(|addr| [format!("{addr}.sslip.io"), format!("{addr}.nip.io")]),
            );
        for name in candidates {
            let Ok(mut addrs) = (name.as_str(), 0).to_socket_addrs() else {
                continue;
            };
            let Some(addr) = addrs.find_map(|addr| match addr.ip() {
                IpAddr::V4(v4) if is_real_egress(v4) => Some(v4),
                _ => None,
            }) else {
                continue;
            };
            // Bound and dropped: a port nothing is listening on. Binding also
            // proves the address is this machine's, so the refusal is local.
            let Ok(listener) = TcpListener::bind((addr, 0)) else {
                continue;
            };
            let Ok(local) = listener.local_addr() else {
                continue;
            };
            return Some(Self {
                name,
                addr,
                port: local.port(),
            });
        }
        None
    }

    fn entry(&self) -> String {
        format!("{}:{}", self.name, self.port)
    }

    fn target(&self) -> SocketAddr {
        SocketAddr::new(self.addr.into(), self.port)
    }
}

fn is_real_egress(addr: Ipv4Addr) -> bool {
    !addr.is_loopback() && !addr.is_link_local() && !addr.is_unspecified()
}

/// The address this machine would send from. Connecting a UDP socket picks
/// the route without sending anything.
fn own_address() -> Option<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(addr) if is_real_egress(addr) => Some(addr),
        _ => None,
    }
}

fn discover_or_explain(test: &str) -> Option<Destination> {
    let destination = Destination::discover();
    match &destination {
        Some(destination) => eprintln!(
            "{test}: resolving {} to {}",
            destination.name, destination.addr
        ),
        None => eprintln!(
            "{test}: skipped — no name resolves to a non-loopback IPv4 address of this \
             machine, so there is no real egress for a guest to resolve its way to"
        ),
    }
    destination
}

fn workload(
    wasm: &'static [u8],
    host_header: &str,
    allowed_hosts: &[String],
    allowed_ip_name_lookups: &[String],
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host_header.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: format!("{host_header}.wasm"),
                digest: None,
                bytes: bytes::Bytes::from_static(wasm),
                local_resources: LocalResources {
                    allowed_hosts: allowed_hosts
                        .iter()
                        .map(|entry| entry.parse().expect("a valid allowed-hosts entry"))
                        .collect(),
                    allowed_ip_name_lookups: allowed_ip_name_lookups
                        .iter()
                        .map(|entry| entry.parse().expect("a valid allowed-name entry"))
                        .collect(),
                    ..Default::default()
                },
                pool_size: 1,
                max_invocations: 100,
                max_concurrency: 0,
                ..Default::default()
            }],
            host_interfaces: http_only_host_interfaces(host_header),
            volumes: vec![],
        },
    }
}

async fn get(addr: SocketAddr, host_header: &str, path: &str) -> Result<(u16, String)> {
    let response = timeout(
        Duration::from_secs(30),
        reqwest::Client::new()
            .get(format!("http://{addr}{path}"))
            .header("HOST", host_header)
            .send(),
    )
    .await
    .context(format!("{path} timed out"))?
    .context(format!("{path} failed"))?;
    let status = response.status().as_u16();
    Ok((status, response.text().await?))
}

fn assert_reached_the_network(body: &str, what: &str) {
    assert!(
        body.contains("connect: ") && !body.contains("connect: denied"),
        "{what}: the socket policy should have let this through to the network: {body:?}"
    );
}

fn assert_denied(body: &str, what: &str) {
    assert!(
        body.contains("connect: denied"),
        "{what}: the socket policy should have refused this: {body:?}"
    );
}

/// One `allowedHosts` entry naming a host is the whole grant: the guest may
/// resolve the name and connect to what it resolved, on that port and no
/// other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_name_entry_lets_a_guest_resolve_and_connect_on_p2_and_p3() -> Result<()> {
    let Some(destination) = discover_or_explain("a_name_entry_lets_a_guest_resolve_and_connect")
    else {
        return Ok(());
    };
    for (wasm, preview) in PREVIEWS {
        let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
        let host_header = format!("{preview}-name-grant");
        host.workload_start(workload(wasm, &host_header, &[destination.entry()], &[]))
            .await?;

        // Before any lookup, the address alone is nothing the policy knows.
        let target = destination.target();
        let (status, body) = get(addr, &host_header, &format!("/-?connect={target}")).await?;
        assert_eq!(status, 200, "{preview}: {body}");
        assert_denied(&body, &format!("{preview}, nothing resolved yet"));

        let name = &destination.name;
        let (status, body) = get(addr, &host_header, &format!("/{name}?connect={target}")).await?;
        assert_eq!(
            status, 200,
            "{preview}: an allowedHosts name may be resolved: {body}"
        );
        assert_reached_the_network(&body, &format!("{preview}, resolved then connected"));

        // The entry pins the port.
        let other_port = SocketAddr::new(target.ip(), target.port().wrapping_add(1).max(1));
        let (_, body) = get(addr, &host_header, &format!("/{name}?connect={other_port}")).await?;
        assert_denied(&body, &format!("{preview}, a port the entry does not name"));
    }
    Ok(())
}

/// Being allowed to resolve a name is not being allowed to connect to it:
/// `allowedIpNameLookups` alone resolves and is then refused the socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lookup_grant_alone_does_not_permit_the_socket_on_p2_and_p3() -> Result<()> {
    let Some(destination) = discover_or_explain("a_lookup_grant_alone_does_not_permit_the_socket")
    else {
        return Ok(());
    };
    for (wasm, preview) in PREVIEWS {
        let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
        let host_header = format!("{preview}-lookup-only");
        host.workload_start(workload(
            wasm,
            &host_header,
            &[],
            std::slice::from_ref(&destination.name),
        ))
        .await?;

        let path = format!("/{}?connect={}", destination.name, destination.target());
        let (status, body) = get(addr, &host_header, &path).await?;
        assert_eq!(
            status, 200,
            "{preview}: the lookup itself is permitted: {body}"
        );
        assert_denied(&body, &format!("{preview}, lookup grant only"));
    }
    Ok(())
}

/// An entry for some other host grants neither the lookup nor the socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_entry_for_another_name_grants_nothing_on_p2_and_p3() -> Result<()> {
    let Some(destination) = discover_or_explain("an_entry_for_another_name_grants_nothing") else {
        return Ok(());
    };
    for (wasm, preview) in PREVIEWS {
        let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
        let host_header = format!("{preview}-other-name");
        host.workload_start(workload(
            wasm,
            &host_header,
            &["elsewhere.invalid:5432".to_string()],
            &[],
        ))
        .await?;

        let path = format!("/{}?connect={}", destination.name, destination.target());
        let (status, body) = get(addr, &host_header, &path).await?;
        assert_eq!(status, 403, "{preview}: {body}");
        assert!(body.contains("denied by policy"), "{preview}: {body}");
    }
    Ok(())
}

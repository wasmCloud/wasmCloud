//! The reserved `*.wasmcloud.internal` zone, end to end.
//!
//! `127.0.0.1` means the workload's own virtual network, and nothing in this
//! design changes that. `host.wasmcloud.internal` is the new, explicitly gated
//! way to reach the machine the host runs on.
//!
//! What these pin:
//!   - resolution of the zone never consults `allowedIpNameLookups`, because
//!     the answer never leaves the process — but *reaching* the host still
//!     needs its own grant
//!   - the grant is two-sided: the workload names the port and the host runs
//!     with `--allow-host-loopback`. Either alone is a refusal.
//!   - `allowedHosts: ["*"]` grants the internet, not the machine
//!   - a permitted port with nothing listening reports a connection failure,
//!     not a policy refusal — otherwise a policy bug and a missing service look
//!     identical

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use anyhow::Result;
use tokio::net::TcpListener;

use wash_runtime::engine::Engine;
use wash_runtime::host::allowed_loopback::AllowedLoopbackPort;
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::sockets::policy::{EgressMode, SocketPolicy};
use wash_runtime::types::LocalResources;

mod common;
use common::{component_workload_request, http_only_host_interfaces, req};

const INTERNAL_ZONE_WASM: &[u8] = include_bytes!("wasm/http_internal_zone.wasm");

/// Start a host whose engine carries `policy`, with a DevRouter ingress.
async fn start_host(policy: SocketPolicy) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder()
        .with_socket_policy(Arc::new(policy))
        .build()?;
    let ingress = wash_runtime::host::http::Ingress::new(
        wash_runtime::host::http::DevRouter::default(),
        "127.0.0.1:0".parse()?,
    )
    .await?;
    let addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .build()?;
    Ok((addr, host.start().await?))
}

fn resources(loopback: Vec<AllowedLoopbackPort>) -> LocalResources {
    LocalResources {
        // Deliberately allow-all for HTTP: the point of several of these tests
        // is that `*` does not reach the machine's own loopback.
        allowed_hosts: vec!["*".parse().unwrap()].into(),
        allowed_host_loopback: loopback.into(),
        ..Default::default()
    }
}

/// A host that permits the loopback door, and a workload granted `port`.
fn granting(port: u16) -> (SocketPolicy, LocalResources) {
    (
        SocketPolicy {
            host_loopback_enabled: true,
            egress_mode: EgressMode::Enforce,
            ..Default::default()
        },
        resources(vec![AllowedLoopbackPort::tcp(port)]),
    )
}

async fn probe(
    addr: &std::net::SocketAddr,
    host: &str,
    name: &str,
    port: u16,
) -> Result<(u16, String)> {
    let client = reqwest::Client::new();
    let (status, body) = req(&client, addr, host, &format!("/{name}/{port}")).await?;
    Ok((status.as_u16(), body))
}

/// Both halves of the grant present: the workload reaches a real listener on
/// the machine's loopback.
#[tokio::test(flavor = "multi_thread")]
async fn the_host_loopback_is_reachable_when_both_sides_grant_it() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            if listener.accept().await.is_err() {
                break;
            }
        }
    });

    let (policy, resources) = granting(port);
    let (addr, h) = start_host(policy).await?;
    h.workload_start(component_workload_request(
        "internal-zone",
        "zone",
        INTERNAL_ZONE_WASM,
        resources,
        http_only_host_interfaces("zone"),
    ))
    .await?;

    let (status, body) = probe(&addr, "zone", "host.wasmcloud.internal", port).await?;
    assert_eq!(status, 200, "expected a connection, got: {body}");
    Ok(())
}

/// The workload named the port but the host never opened the door.
#[tokio::test(flavor = "multi_thread")]
async fn the_workload_grant_alone_is_inert() -> Result<()> {
    let (_, resources) = granting(5432);
    let (addr, h) = start_host(SocketPolicy {
        host_loopback_enabled: false,
        egress_mode: EgressMode::Enforce,
        ..Default::default()
    })
    .await?;
    h.workload_start(component_workload_request(
        "internal-zone",
        "zone",
        INTERNAL_ZONE_WASM,
        resources,
        http_only_host_interfaces("zone"),
    ))
    .await?;

    let (status, body) = probe(&addr, "zone", "host.wasmcloud.internal", 5432).await?;
    assert_eq!(status, 403, "expected a policy refusal, got: {body}");
    Ok(())
}

/// The host opened the door but the workload named no port — and `*` in
/// `allowedHosts` must not stand in for one.
#[tokio::test(flavor = "multi_thread")]
async fn the_host_flag_alone_is_inert_and_star_does_not_substitute() -> Result<()> {
    let (policy, _) = granting(5432);
    let (addr, h) = start_host(policy).await?;
    h.workload_start(component_workload_request(
        "internal-zone",
        "zone",
        INTERNAL_ZONE_WASM,
        resources(vec![]),
        http_only_host_interfaces("zone"),
    ))
    .await?;

    let (status, body) = probe(&addr, "zone", "host.wasmcloud.internal", 5432).await?;
    assert_eq!(
        status, 403,
        "`allowedHosts: [*]` must not grant the machine's own loopback, got: {body}"
    );
    Ok(())
}

/// A grant is per port: naming one does not open its neighbour.
#[tokio::test(flavor = "multi_thread")]
async fn the_grant_does_not_extend_to_other_ports() -> Result<()> {
    let (policy, resources) = granting(5432);
    let (addr, h) = start_host(policy).await?;
    h.workload_start(component_workload_request(
        "internal-zone",
        "zone",
        INTERNAL_ZONE_WASM,
        resources,
        http_only_host_interfaces("zone"),
    ))
    .await?;

    let (status, body) = probe(&addr, "zone", "host.wasmcloud.internal", 6379).await?;
    assert_eq!(status, 403, "expected a policy refusal, got: {body}");
    Ok(())
}

/// A permitted port with nothing behind it must look like a failed connection,
/// not a refusal — otherwise a policy bug is indistinguishable from a service
/// that is simply down.
#[tokio::test(flavor = "multi_thread")]
async fn a_permitted_port_with_no_listener_is_not_a_policy_refusal() -> Result<()> {
    // Bind and release, so the port is almost certainly free.
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);

    let (policy, resources) = granting(port);
    let (addr, h) = start_host(policy).await?;
    h.workload_start(component_workload_request(
        "internal-zone",
        "zone",
        INTERNAL_ZONE_WASM,
        resources,
        http_only_host_interfaces("zone"),
    ))
    .await?;

    let (status, body) = probe(&addr, "zone", "host.wasmcloud.internal", port).await?;
    assert_eq!(
        status, 502,
        "a permitted-but-dead port must report a connection failure, got: {body}"
    );
    Ok(())
}

/// Resolution inside the zone bypasses `allowedIpNameLookups` — the answer
/// never leaves the process, so that allowlist has nothing to protect. The
/// refusal a workload without a loopback grant sees comes from connect.
#[tokio::test(flavor = "multi_thread")]
async fn zone_resolution_does_not_need_a_lookup_grant() -> Result<()> {
    let (policy, _) = granting(5432);
    let (addr, h) = start_host(policy).await?;
    h.workload_start(component_workload_request(
        "internal-zone",
        "zone",
        INTERNAL_ZONE_WASM,
        LocalResources {
            allowed_hosts: vec!["*".parse().unwrap()].into(),
            // Empty: every ordinary name lookup is denied.
            allowed_ip_name_lookups: Default::default(),
            allowed_host_loopback: vec![AllowedLoopbackPort::tcp(5432)].into(),
            ..Default::default()
        },
        http_only_host_interfaces("zone"),
    ))
    .await?;

    // Resolution succeeded (no 403-from-resolution) and we got as far as
    // connecting, which fails because nothing is listening.
    let (status, body) = probe(&addr, "zone", "host.wasmcloud.internal", 5432).await?;
    assert_eq!(
        status, 502,
        "zone resolution must not require a lookup grant, got: {body}"
    );

    // An ordinary name is still denied at resolution, proving the allowlist is
    // genuinely empty rather than accidentally permissive.
    let (status, body) = probe(&addr, "zone", "example.com", 80).await?;
    assert_eq!(
        status, 403,
        "an ordinary name must still need a lookup grant, got: {body}"
    );
    Ok(())
}

/// An undefined name inside the zone must fail rather than fall through to
/// DNS, or a cluster running a real `wasmcloud.internal` zone could answer for
/// it.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_name_in_the_zone_does_not_reach_dns() -> Result<()> {
    let (policy, resources) = granting(5432);
    let (addr, h) = start_host(policy).await?;
    h.workload_start(component_workload_request(
        "internal-zone",
        "zone",
        INTERNAL_ZONE_WASM,
        resources,
        http_only_host_interfaces("zone"),
    ))
    .await?;

    let (status, body) = probe(&addr, "zone", "nope.wasmcloud.internal", 80).await?;
    assert_eq!(status, 404, "expected name-unresolvable, got: {body}");
    Ok(())
}

/// `service.wasmcloud.internal` names the workload's own virtual network, so
/// it needs no grant at all — and with no service listening it is refused by
/// the virtual network rather than by policy.
#[tokio::test(flavor = "multi_thread")]
async fn the_service_name_resolves_into_the_virtual_network() -> Result<()> {
    let (addr, h) = start_host(SocketPolicy {
        egress_mode: EgressMode::Enforce,
        ..Default::default()
    })
    .await?;
    h.workload_start(component_workload_request(
        "internal-zone",
        "zone",
        INTERNAL_ZONE_WASM,
        // No `allowedHostLoopback`, and an empty `allowedHosts`: the workload's
        // own virtual network is reachable regardless.
        LocalResources {
            allowed_hosts: Default::default(),
            ..Default::default()
        },
        http_only_host_interfaces("zone"),
    ))
    .await?;

    let (status, body) = probe(&addr, "zone", "service.wasmcloud.internal", 8080).await?;
    assert_eq!(
        status, 502,
        "the virtual network should refuse the connection, not the policy, got: {body}"
    );
    Ok(())
}

/// Named-service syntax is reserved now so manifests written against it keep
/// working when a workload can have several services.
#[tokio::test(flavor = "multi_thread")]
async fn the_named_service_spelling_is_accepted() -> Result<()> {
    let (addr, h) = start_host(SocketPolicy {
        egress_mode: EgressMode::Enforce,
        ..Default::default()
    })
    .await?;
    h.workload_start(component_workload_request(
        "internal-zone",
        "zone",
        INTERNAL_ZONE_WASM,
        LocalResources::default(),
        http_only_host_interfaces("zone"),
    ))
    .await?;

    let (status, body) = probe(&addr, "zone", "api.svc.wasmcloud.internal", 8080).await?;
    assert_eq!(status, 502, "expected to reach connect, got: {body}");
    Ok(())
}

/// `127.0.0.1` keeps meaning the virtual network. Nothing about the zone
/// changes what already worked, and this is the test that would catch it if a
/// future change flipped the default.
#[tokio::test(flavor = "multi_thread")]
async fn plain_loopback_still_means_the_virtual_network() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            if listener.accept().await.is_err() {
                break;
            }
        }
    });

    // Grant that exact port on the host-loopback door — which must NOT make
    // plain `127.0.0.1` reach it, because the door is the sentinel's alone.
    let (policy, resources) = granting(port);
    let (addr, h) = start_host(policy).await?;
    h.workload_start(component_workload_request(
        "internal-zone",
        "zone",
        INTERNAL_ZONE_WASM,
        LocalResources {
            // A literal address is still a name lookup as far as
            // `resolve-addresses` is concerned, so it needs its own grant —
            // otherwise this test would pass for the wrong reason, refused at
            // resolution before connect ever ran.
            allowed_ip_name_lookups: vec!["127.0.0.1".parse().unwrap()].into(),
            ..resources
        },
        http_only_host_interfaces("zone"),
    ))
    .await?;

    let (status, body) = probe(&addr, "zone", "127.0.0.1", port).await?;
    assert_eq!(
        status, 502,
        "127.0.0.1 must stay virtual even when the same port is granted on the \
         host-loopback door, got: {body}"
    );
    Ok(())
}

/// `wasi:http` to the reserved zone, which the sockets tests above cannot
/// cover: a different code path, with its own policy checks, reached through
/// `CtxHttpHooks` rather than `SocketAddrCheck`.
mod over_http {
    use super::*;

    const INTERNAL_FETCH_WASM: &[u8] = include_bytes!("wasm/http_internal_fetch.wasm");

    /// Serve one trivial HTTP response on the machine's real loopback.
    async fn host_http_server() -> Result<(u16, tokio::task::JoinHandle<()>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let handle = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(|_req| async {
                        Ok::<_, std::convert::Infallible>(hyper::Response::new(
                            http_body_util::Full::new(bytes::Bytes::from_static(b"from the host")),
                        ))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Ok((port, handle))
    }

    fn fetch_workload(resources: LocalResources) -> wash_runtime::types::WorkloadStartRequest {
        component_workload_request(
            "internal-fetch",
            "fetch",
            INTERNAL_FETCH_WASM,
            resources,
            http_only_host_interfaces("fetch"),
        )
    }

    /// Both halves of the grant, plus the name listed in `allowedHosts`.
    #[tokio::test(flavor = "multi_thread")]
    async fn http_reaches_the_host_loopback_when_granted() -> Result<()> {
        let (port, _server) = host_http_server().await?;
        let (addr, h) = start_host(SocketPolicy {
            host_loopback_enabled: true,
            egress_mode: EgressMode::Enforce,
            ..Default::default()
        })
        .await?;
        h.workload_start(fetch_workload(LocalResources {
            allowed_hosts: vec![format!("host.wasmcloud.internal:{port}").parse().unwrap()].into(),
            allowed_host_loopback: vec![AllowedLoopbackPort::tcp(port)].into(),
            ..Default::default()
        }))
        .await?;

        let client = reqwest::Client::new();
        let (status, body) = req(
            &client,
            &addr,
            "fetch",
            &format!("/host.wasmcloud.internal:{port}"),
        )
        .await?;
        assert_eq!(status.as_u16(), 200, "expected to reach the host: {body}");
        Ok(())
    }

    /// `*` grants the internet, not the machine — the carve-out in
    /// `check_allowed_hosts` has to hold on the HTTP path too.
    #[tokio::test(flavor = "multi_thread")]
    async fn star_in_allowed_hosts_does_not_grant_the_host_loopback() -> Result<()> {
        let (port, _server) = host_http_server().await?;
        let (addr, h) = start_host(SocketPolicy {
            host_loopback_enabled: true,
            egress_mode: EgressMode::Enforce,
            ..Default::default()
        })
        .await?;
        h.workload_start(fetch_workload(LocalResources {
            allowed_hosts: vec!["*".parse().unwrap()].into(),
            // The loopback grant is present; only the `allowedHosts` naming is
            // missing, so this isolates the carve-out.
            allowed_host_loopback: vec![AllowedLoopbackPort::tcp(port)].into(),
            ..Default::default()
        }))
        .await?;

        let client = reqwest::Client::new();
        let (status, body) = req(
            &client,
            &addr,
            "fetch",
            &format!("/host.wasmcloud.internal:{port}"),
        )
        .await?;
        assert_eq!(
            status.as_u16(),
            403,
            "`*` must not grant the reserved zone: {body}"
        );
        Ok(())
    }

    /// Named in `allowedHosts` but with no `allowedHostLoopback` entry: the
    /// second half of the grant is genuinely required on this path too.
    #[tokio::test(flavor = "multi_thread")]
    async fn http_needs_the_loopback_grant_as_well_as_the_name() -> Result<()> {
        let (port, _server) = host_http_server().await?;
        let (addr, h) = start_host(SocketPolicy {
            host_loopback_enabled: true,
            egress_mode: EgressMode::Enforce,
            ..Default::default()
        })
        .await?;
        h.workload_start(fetch_workload(LocalResources {
            allowed_hosts: vec![format!("host.wasmcloud.internal:{port}").parse().unwrap()].into(),
            allowed_host_loopback: Default::default(),
            ..Default::default()
        }))
        .await?;

        let client = reqwest::Client::new();
        let (status, body) = req(
            &client,
            &addr,
            "fetch",
            &format!("/host.wasmcloud.internal:{port}"),
        )
        .await?;
        assert_eq!(status.as_u16(), 403, "expected a policy refusal: {body}");
        Ok(())
    }

    /// A workload with no service has nothing listening in its virtual
    /// network, so this must report a transport failure rather than a policy
    /// refusal — and must not fall through to the machine's port 80.
    #[tokio::test(flavor = "multi_thread")]
    async fn http_to_the_service_name_stays_inside_the_workload() -> Result<()> {
        let (addr, h) = start_host(SocketPolicy {
            egress_mode: EgressMode::Enforce,
            ..Default::default()
        })
        .await?;
        h.workload_start(fetch_workload(LocalResources {
            allowed_hosts: vec!["service.wasmcloud.internal:8080".parse().unwrap()].into(),
            ..Default::default()
        }))
        .await?;

        let client = reqwest::Client::new();
        let (status, body) =
            req(&client, &addr, "fetch", "/service.wasmcloud.internal:8080").await?;
        assert_eq!(
            status.as_u16(),
            502,
            "expected a transport failure from the empty virtual network: {body}"
        );
        assert!(
            body.contains("nothing is listening"),
            "the error should say what is missing: {body}"
        );
        Ok(())
    }
}

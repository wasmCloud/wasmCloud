//! Integration tests for the per-workload `allowed_ip_name_lookups` policy on
//! `wasi:sockets/ip-name-lookup` (`resolve-addresses`).
//!
//! Uses the http-ip-name-lookup (wasip2) and http-ip-name-lookup-p3 (wasip3)
//! fixtures, which resolve the name given as the request path and report the
//! host's decision via their status code:
//!
//! - 200 OK: lookup permitted, body reports the address count
//! - 403 Forbidden: denied by the host (`permanent-resolver-failure`)
//! - 502 Bad Gateway: any other resolution error
//!
//! The policy lives in [`LocalResources`], so these pin that an undeclared
//! policy denies, that a declared one admits only the names it names, and
//! that both hold on the p2 path (the `network` resource snapshot in
//! `sockets/host_instance_network.rs`) and the p3 path (the ctx read in
//! `sockets/host_ip_name_lookup_p3.rs`).
//!
//! Denial is per name rather than per interface, which is what closes the
//! channel where a component resolves attacker-chosen labels to carry data
//! off the host. [`test_wildcard_admits_only_its_suffix`] is that case.
//!
//! Every test sends more requests than the component's `max_invocations`, so
//! warm pooled instances are retired and rebuilt mid-test: the policy has to
//! survive instance reuse *and* the fresh-instance rebuild from the ctx
//! template.
//!
//! `/127.0.0.1` resolves a literal address, which the host answers without
//! consulting any resolver, keeping the assertion hermetic. `/localhost`
//! additionally walks the real `getaddrinfo` path via the hosts file.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::{Context, Result};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use common::{
    http_only_host_interfaces, start_host_with_dynamic_router, start_host_with_p3_http_handler,
};
use wash_runtime::{
    host::HostApi,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

const HTTP_IP_NAME_LOOKUP_WASM: &[u8] = include_bytes!("wasm/http_ip_name_lookup.wasm");
const HTTP_IP_NAME_LOOKUP_P3_WASM: &[u8] = include_bytes!("wasm/http_ip_name_lookup_p3.wasm");

const PREVIEWS: &[(&[u8], &str)] = &[
    (HTTP_IP_NAME_LOOKUP_WASM, "p2"),
    (HTTP_IP_NAME_LOOKUP_P3_WASM, "p3"),
];

/// How many calls a warm instance serves before it is retired. Kept below
/// [`REQUESTS`] so every test crosses a retirement boundary.
const MAX_INVOCATIONS: i32 = 2;
/// Requests per test: enough for warm reuse (2nd call on an instance) and
/// at least two retire-and-rebuild cycles.
const REQUESTS: usize = 6;

fn resolve_workload(
    wasm: &'static [u8],
    host_header: &str,
    allowed_ip_name_lookups: &[&str],
) -> WorkloadStartRequest {
    let parsed: Vec<wash_runtime::host::allowed_ip_name::AllowedIpName> = allowed_ip_name_lookups
        .iter()
        .map(|s| s.parse().expect("test gave an invalid allowed-name entry"))
        .collect();
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
                    allowed_ip_name_lookups: parsed.into(),
                    ..Default::default()
                },
                pool_size: 1,
                max_invocations: MAX_INVOCATIONS,
            }],
            host_interfaces: http_only_host_interfaces(host_header),
            volumes: vec![],
        },
    }
}

/// GETs `path` [`REQUESTS`] times and asserts every response carries `status`
/// and contains `body_marker`, across warm-instance reuse and retirement.
async fn assert_repeated(
    addr: std::net::SocketAddr,
    host_header: &str,
    path: &str,
    status: u16,
    body_marker: &str,
) -> Result<()> {
    let client = reqwest::Client::new();
    for i in 0..REQUESTS {
        let response = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}{path}"))
                .header("HOST", host_header)
                .send(),
        )
        .await
        .context(format!("{path} request {i} timed out"))?
        .context(format!("{path} request {i} failed"))?;

        assert_eq!(
            response.status().as_u16(),
            status,
            "{path} request {i} (instance retires every {MAX_INVOCATIONS} calls)"
        );
        let body = response.text().await?;
        assert!(
            body.contains(body_marker),
            "{path} request {i}: body {body:?} should contain {body_marker:?}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_lookup_denied_when_no_policy_declared_on_p2_and_p3() -> Result<()> {
    for (wasm, preview) in PREVIEWS {
        let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
        let host_header = format!("{preview}-lookup-deny");
        let req = resolve_workload(wasm, &host_header, &[]);
        host.workload_start(req).await?;

        assert_repeated(addr, &host_header, "/127.0.0.1", 403, "denied by policy").await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_lookup_allowed_by_star_on_p2_and_p3() -> Result<()> {
    for (wasm, preview) in PREVIEWS {
        let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
        let host_header = format!("{preview}-lookup-star");
        let req = resolve_workload(wasm, &host_header, &["*"]);
        host.workload_start(req).await?;

        assert_repeated(addr, &host_header, "/127.0.0.1", 200, "1 addresses").await?;
        assert_repeated(addr, &host_header, "/localhost", 200, "addresses").await?;
    }
    Ok(())
}

/// A policy naming one address admits that address and refuses every other
/// name, on both previews. Without per-name matching, the second case here
/// would resolve and the lookup would be an open channel off the host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_policy_admits_only_the_names_it_lists() -> Result<()> {
    for (wasm, host_header) in [
        (HTTP_IP_NAME_LOOKUP_WASM, "p2-lookup-listed"),
        (HTTP_IP_NAME_LOOKUP_P3_WASM, "p3-lookup-listed"),
    ] {
        let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
        let req = resolve_workload(wasm, host_header, &["127.0.0.1"]);
        host.workload_start(req).await?;

        assert_repeated(addr, host_header, "/127.0.0.1", 200, "1 addresses").await?;
        assert_repeated(
            addr,
            host_header,
            "/leak.attacker.example",
            403,
            "denied by policy",
        )
        .await?;
    }
    Ok(())
}

/// A wildcard admits its subdomains and nothing else. The denied cases are
/// the shapes a guest would reach for to smuggle labels past a suffix
/// policy: an unrelated domain, the bare suffix, a name that merely ends
/// with the suffix text, and the suffix buried in an attacker-owned parent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_wildcard_admits_only_its_suffix() -> Result<()> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    let req = resolve_workload(
        HTTP_IP_NAME_LOOKUP_WASM,
        "lookup-wildcard",
        &["*.localhost"],
    );
    host.workload_start(req).await?;

    assert_repeated(addr, "lookup-wildcard", "/sub.localhost", 200, "addresses").await?;

    for path in [
        "/evil.example",
        "/localhost",
        "/notlocalhost",
        "/localhost.attacker.example",
    ] {
        assert_repeated(addr, "lookup-wildcard", path, 403, "denied by policy").await?;
    }
    Ok(())
}

/// Two workloads on one host with different policies: the policy is scoped
/// to the workload, not the host. Needs the
/// [`DynamicRouter`](wash_runtime::host::http::DynamicRouter), since the
/// `DevRouter` the other tests use routes every request to the sole
/// workload and ignores the HOST header these two are told apart by.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_policy_is_per_workload() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    host.workload_start(resolve_workload(
        HTTP_IP_NAME_LOOKUP_WASM,
        "lookup-granted",
        &["127.0.0.1"],
    ))
    .await?;
    host.workload_start(resolve_workload(
        HTTP_IP_NAME_LOOKUP_WASM,
        "lookup-ungranted",
        &[],
    ))
    .await?;

    let client = reqwest::Client::new();
    for i in 0..REQUESTS {
        for (host_header, status, marker) in [
            ("lookup-granted", 200, "1 addresses"),
            ("lookup-ungranted", 403, "denied by policy"),
        ] {
            let response = timeout(
                Duration::from_secs(10),
                client
                    .get(format!("http://{addr}/127.0.0.1"))
                    .header("HOST", host_header)
                    .send(),
            )
            .await
            .context(format!("{host_header} request {i} timed out"))?
            .context(format!("{host_header} request {i} failed"))?;

            assert_eq!(
                response.status().as_u16(),
                status,
                "{host_header} request {i}"
            );
            let body = response.text().await?;
            assert!(
                body.contains(marker),
                "{host_header} request {i}: body {body:?} should contain {marker:?}"
            );
        }
    }
    Ok(())
}

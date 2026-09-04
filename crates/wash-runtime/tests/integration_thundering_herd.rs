//! Integration tests for a scheduling thundering herd: a whole deployment's
//! worth of workloads arriving at one host at once.
//!
//! A scheduler placing many workloads on one host issues every
//! `workload_start` together, and each one compiles a component, binds its
//! plugins and registers its route before it can report `Running`. These tests
//! pin that the host admits the whole herd rather than losing, mis-binding or
//! serialising part of it.
//!
//! Driven through [`HostApi`] rather than the washlet, so the whole herd
//! arrives at once: the washlet's `max_concurrent_starts` permit is what caps
//! this in a deployed host, and it is covered by
//! `integration_washlet_api::concurrent_workload_starts_are_bounded`. What is
//! left to pin is what the host does when a herd is let through uncapped.
//!
//! Covers:
//! - 15 concurrent starts of distinct workloads: every one reports `Running`,
//!   every one is individually reachable over HTTP, and the host counts
//!   exactly the herd.
//! - Stopping all 15 under the same concurrency: every route goes, every id
//!   goes, and the host is left empty.
//! - A workload already serving traffic keeps answering throughout the herd,
//!   in a fraction of the time the herd itself takes — compilation must not
//!   occupy the runtime the HTTP server is on.
//! - 15 concurrent starts of the *same* workload id: exactly one is admitted
//!   and the other fourteen are refused, with nothing left half-started.
//!
//! Every herd here carries no digest, so no two members share a compile: this
//! is the cost of a herd whose components are all different, which is the
//! worse of the two shapes by a wide margin. The other shape — N replicas of
//! one image, which share one compile through the digest-keyed cache — is
//! pinned by `engine::tests::a_herd_of_replicas_shares_one_compiled_component`.

use anyhow::{Context, Result};
use futures::future::join_all;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wash_runtime::{
    host::HostApi,
    types::{
        WorkloadStartRequest, WorkloadStartResponse, WorkloadState, WorkloadStatusRequest,
        WorkloadStopRequest,
    },
};

mod common;
use common::{
    component_workload_request, default_counter_resources, get_status,
    http_counter_host_interfaces, start_host_with_dynamic_router,
};

const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");

/// Enough workloads for the herd to cost something real: fifteen components to
/// compile, fifteen plugin bindings and fifteen route registrations, all
/// issued at once.
const HERD: usize = 15;

/// The least a request to an already-serving workload is allowed, however fast
/// the herd was. A herd that finishes in well under a second leaves too small
/// a share to hold anything to.
const RESPONSIVE_FLOOR: Duration = Duration::from_secs(1);

fn herd_host(i: usize) -> String {
    format!("herd-{i}.local")
}

/// A member of the herd carrying no digest, so it compiles on its own — the
/// cost of a herd whose components are all different.
fn herd_request(host_header: &str) -> WorkloadStartRequest {
    component_workload_request(
        "http-counter.wasm",
        "herd-workload",
        HTTP_COUNTER_WASM,
        default_counter_resources(),
        http_counter_host_interfaces(host_header),
    )
}

/// Report every start that did not reach `Running`, named by its position in
/// the herd, so a failure says which ones fell out and why.
fn not_running(responses: Vec<Result<WorkloadStartResponse>>) -> Vec<String> {
    responses
        .into_iter()
        .enumerate()
        .filter_map(|(i, response)| match response {
            Ok(response) if response.workload_status.workload_state == WorkloadState::Running => {
                None
            }
            Ok(response) => Some(format!(
                "{i}: {:?} — {}",
                response.workload_status.workload_state, response.workload_status.message
            )),
            Err(e) => Some(format!("{i}: start errored — {e:#}")),
        })
        .collect()
}

/// Fifteen distinct workloads started at once must all come up, all serve, and
/// all stop — under the same concurrency they started with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_herd_starts_all_run_and_serve() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;

    let requests: Vec<WorkloadStartRequest> =
        (0..HERD).map(|i| herd_request(&herd_host(i))).collect();
    let ids: Vec<String> = requests.iter().map(|r| r.workload_id.clone()).collect();

    let started = Instant::now();
    let responses = join_all(requests.into_iter().map(|r| host.workload_start(r))).await;
    eprintln!(
        "{HERD} concurrent starts settled in {:?}",
        started.elapsed()
    );

    let failures = not_running(responses);
    assert!(
        failures.is_empty(),
        "{} of {HERD} concurrent starts did not run: {failures:?}",
        failures.len()
    );

    // `Running` is what each start reported; this is what the host still holds.
    for (i, id) in ids.iter().enumerate() {
        let status = host
            .workload_status(WorkloadStatusRequest {
                workload_id: id.clone(),
            })
            .await?
            .workload_status;
        assert_eq!(
            status.workload_state,
            WorkloadState::Running,
            "workload {i} ({id}) is {:?} after the herd: {}",
            status.workload_state,
            status.message
        );
    }
    assert_eq!(host.heartbeat().await?.workload_count, HERD as u64);

    // Bound and routed, not just bookkept: every member of the herd answers on
    // its own hostname.
    let client = reqwest::Client::new();
    let hostnames: Vec<String> = (0..HERD).map(herd_host).collect();
    let served = join_all(
        hostnames
            .iter()
            .map(|hostname| get_status(&client, addr, hostname)),
    )
    .await;
    for (i, status) in served.into_iter().enumerate() {
        assert_eq!(
            status.with_context(|| format!("request to {} failed", herd_host(i)))?,
            reqwest::StatusCode::OK,
            "workload {i} did not serve after the herd"
        );
    }

    // Tear the herd down the way it came up.
    let stopped = join_all(ids.iter().map(|id| {
        host.workload_stop(WorkloadStopRequest {
            workload_id: id.clone(),
        })
    }))
    .await;
    for (i, response) in stopped.into_iter().enumerate() {
        assert_eq!(
            response?.workload_status.workload_state,
            WorkloadState::Stopping,
            "workload {i} did not stop cleanly"
        );
    }

    for (i, id) in ids.iter().enumerate() {
        assert_eq!(
            host.workload_status(WorkloadStatusRequest {
                workload_id: id.clone(),
            })
            .await?
            .workload_status
            .workload_state,
            WorkloadState::NotFound,
            "workload {i} ({id}) outlived its stop"
        );
    }
    assert_eq!(host.heartbeat().await?.workload_count, 0);

    // Every route went with its workload.
    let after = join_all(
        hostnames
            .iter()
            .map(|hostname| get_status(&client, addr, hostname)),
    )
    .await;
    for (i, status) in after.into_iter().enumerate() {
        assert_eq!(
            status?,
            reqwest::StatusCode::NOT_FOUND,
            "workload {i}'s route outlived its stop"
        );
    }

    Ok(())
}

/// The herd must not take the host off the air. Compiling fifteen components
/// occupies fifteen blocking threads; run on the runtime instead, it would
/// stop the HTTP server answering for as long as the herd took.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_herd_keeps_a_running_workload_serving() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;

    host.workload_start(herd_request("steady.local")).await?;
    let client = reqwest::Client::new();
    assert_eq!(
        get_status(&client, addr, "steady.local").await?,
        reqwest::StatusCode::OK,
        "the steady workload must serve before the herd arrives"
    );

    let herd_done = Arc::new(AtomicBool::new(false));
    let requests: Vec<WorkloadStartRequest> =
        (0..HERD).map(|i| herd_request(&herd_host(i))).collect();

    let herd = async {
        let started = Instant::now();
        let responses = join_all(requests.into_iter().map(|r| host.workload_start(r))).await;
        let took = started.elapsed();
        herd_done.store(true, Ordering::SeqCst);
        (responses, took)
    };

    // A request is kept in flight for the whole herd, so a runtime that stalls
    // is measured by a request already waiting on it.
    let steady = async {
        let mut worst = Duration::ZERO;
        let mut served = 0u32;
        while !herd_done.load(Ordering::SeqCst) {
            let sent = Instant::now();
            let status = get_status(&client, addr, "steady.local").await?;
            let took = sent.elapsed();
            assert_eq!(
                status,
                reqwest::StatusCode::OK,
                "the steady workload stopped serving during the herd"
            );
            worst = worst.max(took);
            served += 1;
        }
        anyhow::Ok((worst, served))
    };

    let ((responses, herd_took), steady) = tokio::join!(herd, steady);
    let (worst, served) = steady?;
    eprintln!(
        "herd took {herd_took:?}; steady workload served {served} requests during it, worst {worst:?}"
    );

    let failures = not_running(responses);
    assert!(
        failures.is_empty(),
        "{} of {HERD} concurrent starts did not run: {failures:?}",
        failures.len()
    );
    // How many requests got through is the sharp measure, and it needs no
    // fixed budget: a slower machine takes longer over the herd, which is more
    // time to serve, not less. Compiling on the runtime instead drops this to
    // single digits — the requests that happen to be in flight between one
    // compile and the next.
    assert!(
        served as usize >= HERD,
        "the steady workload served only {served} requests across a {herd_took:?} herd"
    );
    // Second reading of the same thing, from the other end: no one request may
    // spend anything like the herd waiting for a runtime the herd has taken
    // over.
    let ceiling = (herd_took / 2).max(RESPONSIVE_FLOOR);
    assert!(
        worst < ceiling,
        "the steady workload took {worst:?} to answer during a {herd_took:?} herd, over the {ceiling:?} ceiling"
    );

    Ok(())
}

/// A scheduler retrying a start it already issued sends the same id again.
/// Fifteen of those at once must leave exactly one workload, not fifteen
/// half-bound ones sharing an id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_herd_of_one_id_admits_exactly_one() -> Result<()> {
    let (_addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;

    let template = herd_request("duplicate.local");
    let requests: Vec<WorkloadStartRequest> = (0..HERD)
        .map(|_| WorkloadStartRequest {
            workload_id: template.workload_id.clone(),
            workload: template.workload.clone(),
        })
        .collect();

    let responses = join_all(requests.into_iter().map(|r| host.workload_start(r)))
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;

    let states: Vec<WorkloadState> = responses
        .into_iter()
        .map(|r| r.workload_status.workload_state)
        .collect();
    assert_eq!(
        states
            .iter()
            .filter(|state| **state == WorkloadState::Running)
            .count(),
        1,
        "exactly one of {HERD} starts of one id may be admitted, got {states:?}"
    );
    assert_eq!(
        states
            .iter()
            .filter(|state| **state == WorkloadState::Error)
            .count(),
        HERD - 1,
        "every start that lost the race must be refused, got {states:?}"
    );
    assert_eq!(host.heartbeat().await?.workload_count, 1);

    Ok(())
}

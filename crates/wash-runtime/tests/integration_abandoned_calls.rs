//! What ends a guest that never yields, at every ingress.
//!
//! Every other timeout in the runtime is a future the host awaits, which works
//! for a guest parked in a host call — `integration_http_call_timeout`'s
//! `/wedge` is suspended, so control is back in the executor and the timer
//! fires. A guest that *spins* is the case none of them reach: its poll never
//! returns, so no timer on that task is ever polled and no host code on that
//! store runs again. What ends it is abandonment (`wash_runtime::engine::
//! abandon`): the dispatcher enforcing the call's deadline runs *outside* the
//! store, arms the call's flag when it stops wanting the result, and the epoch
//! callback compiled into the guest's own loop back-edges — the only host code
//! a spinning guest cannot block — acts once the call stays abandoned past its
//! grace.
//!
//! What that costs depends on the store:
//!
//!  * A **pooled instance** or **ephemeral store** is trapped; the pool reaps
//!    it and the next call gets a fresh one.
//!  * A **service** is trapped too; its supervisor restarts it — a restart
//!    beats carrying a wedged call for the rest of the singleton's life.
//!  * A **host component plugin** is warned about at the grace and trapped
//!    only at the escalation: its one store serves every tenant, so a
//!    yielding abandoned call gets a long runway to finish harmlessly — but a
//!    non-yielding one holds the store's guest execution, wedging every
//!    tenant, and the supervised restart is what restores service.
//!
//! One test per ingress: HTTP to a pooled instance and to a service, a linked
//! call to a cold ephemeral store and to a pooled instance, a message delivery,
//! and a capability call into a plugin. The counterweight test at the bottom
//! drives steady healthy traffic that must never be touched: with no CPU
//! budget anywhere, a store is only ever acted on when a call on it is
//! actually abandoned.
//!
//! These live in their own binary because every deadline here is cached
//! process-wide on first read, and this file wants them all very short.
//!
//! Spin tests need a multi-threaded runtime: a spinning guest occupies the
//! worker thread its store is driven on, so on the single-threaded default it
//! would starve the HTTP server and the test itself.

// `std::env::set_var` is unsafe on edition 2024. The override below runs once,
// before any host is started and before anything else in this process reads
// the environment, which is the soundness condition it needs.
#![allow(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
// Every test holds `abandonment_env`'s serial guard across its awaits by
// design: they pin CPUs and assert on timing, so they must not overlap. An
// async mutex cannot serialise whole `#[tokio::test]` bodies, each on its own
// runtime. Nothing contends the lock from inside an await.
#![allow(clippy::await_holding_lock)]

use std::collections::HashMap;
use std::sync::Once;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, json_u64_field, start_host_with_dynamic_router};

const HTTP_SLEEPER_WASM: &[u8] = include_bytes!("wasm/http_sleeper.wasm");
const EPHEMERAL_CALLER_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_caller_p3.wasm");
const EPHEMERAL_CALLEE_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_callee_p3.wasm");
const MSG_COUNTER_WASM: &[u8] = include_bytes!("wasm/msg_counter.wasm");
#[cfg(feature = "host-component-plugins")]
const KV_PLUGIN_WASM: &[u8] = include_bytes!("wasm/kv_plugin.wasm");
#[cfg(feature = "host-component-plugins")]
const KV_PLUGIN_CALLER_WASM: &[u8] = include_bytes!("wasm/kv_plugin_caller.wasm");

/// Every ingress deadline in this binary. Once it passes the dispatcher
/// abandons the call.
const DEADLINE_SECS: u64 = 2;
/// How long an abandoned call may keep running before its store acts. Short,
/// so traps land quickly; the default (10s) is tuned for production, where a
/// disconnect must not condemn a call that was about to finish.
const GRACE_SECS: u64 = 1;

/// Deadline + grace + epoch ticks + unwind, with a wide margin. A wedged call
/// must fail within this; without abandonment it hangs forever.
const SPIN_BOUND: Duration = Duration::from_secs(20);

/// Set this binary's deadlines and grace. All are cached process-wide on first
/// read, so every test must want the same values and must call this before
/// starting a host.
/// Every test here pins or starves CPUs and then makes timing assertions, so
/// two at once on a small CI runner distort the very timing the assertions
/// depend on. The returned guard runs them one at a time.
fn abandonment_env() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    static SET: Once = Once::new();
    SET.call_once(|| unsafe {
        let deadline = DEADLINE_SECS.to_string();
        std::env::set_var("WASH_HTTP_RESPONSE_TIMEOUT_SECS", &deadline);
        std::env::set_var("WASH_EPHEMERAL_CALL_TIMEOUT_SECS", &deadline);
        std::env::set_var("WASH_MESSAGING_DELIVER_TIMEOUT_SECS", &deadline);
        std::env::set_var("WASH_PLUGIN_CAPABILITY_CALL_TIMEOUT_SECS", &deadline);
        std::env::set_var("WASH_ABANDONED_CALL_GRACE_SECS", GRACE_SECS.to_string());
        std::env::set_var("WASH_ABANDONED_CALL_ESCALATION_SECS", "3");
    });
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(30))
        .build()?)
}

/// Retry `probe` every 250ms until it succeeds or `bound` passes — for the
/// requests right after a trap, which race the pool reap or the service
/// supervisor's restart.
async fn eventually<T, F>(bound: Duration, mut probe: F) -> Result<T>
where
    F: AsyncFnMut() -> Result<T>,
{
    let started = Instant::now();
    loop {
        match probe().await {
            Ok(v) => return Ok(v),
            Err(e) if started.elapsed() > bound => {
                return Err(e.context(format!("probe still failing after {bound:?}")));
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
}

/// Start a `http-sleeper` workload, pooled or as a service, under `name`.
async fn start_sleeper(host: &impl HostApi, name: &str, as_service: bool) -> Result<()> {
    let (service, components) = if as_service {
        (
            Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_SLEEPER_WASM),
                local_resources: LocalResources::default(),
                // A trapped service must come back: that restart is half of
                // what the service test asserts.
                max_restarts: 2,
            }),
            vec![],
        )
    } else {
        (
            None,
            vec![Component {
                name: "sleeper".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_SLEEPER_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 0,
                max_concurrency: 4,
            }],
        )
    };
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: name.to_string(),
            annotations: HashMap::new(),
            service,
            components,
            host_interfaces: http_only_host_interfaces(name),
            volumes: vec![],
        },
    })
    .await
    .with_context(|| format!("sleeper workload {name} should start"))?;
    Ok(())
}

/// Start an `ephemeral-caller-p3` -> `ephemeral-callee-p3` workload under
/// `name`. `callee_pool` picks the linked-call path a `/spin` wedges: `0`
/// serves each call from a cold ephemeral store, `1` from a warm pooled
/// instance.
async fn start_linked(host: &impl HostApi, name: &str, callee_pool: i32) -> Result<()> {
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: name.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "ephemeral-caller".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(EPHEMERAL_CALLER_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 0,
                    max_concurrency: 4,
                },
                Component {
                    name: "ephemeral-callee".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(EPHEMERAL_CALLEE_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: callee_pool,
                    max_invocations: 0,
                    max_concurrency: 4,
                },
            ],
            host_interfaces: http_only_host_interfaces(name),
            volumes: vec![],
        },
    })
    .await
    .with_context(|| format!("linked workload {name} should start"))?;
    Ok(())
}

/// GET `path` and return `(status, body)`.
async fn get(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host_header: &str,
    path: &str,
) -> Result<(reqwest::StatusCode, String)> {
    let resp = client
        .get(format!("http://{addr}{path}"))
        .header("HOST", host_header)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    Ok((status, body))
}

/// GET `/` and return the instance's `served` count.
async fn served(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host_header: &str,
) -> Result<u64> {
    let (status, body) = get(client, addr, host_header, "/").await?;
    anyhow::ensure!(status.is_success(), "status {status}");
    Ok(json_u64_field(&body, "served"))
}

/// A pooled guest that never yields is ended by abandonment and its instance
/// replaced. No host-side timeout can reach this call — the fixture's `/spin`
/// runs forever and never lets one be polled — so the request completing at all
/// is the epoch deadline doing it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spinning_pooled_instance_is_trapped_and_replaced() -> Result<()> {
    let _serial = abandonment_env();
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    start_sleeper(&host, "spin-pooled", false).await?;
    let client = client()?;

    // Warm the one instance; its own count reads 1.
    assert_eq!(served(&client, addr, "spin-pooled").await?, 1);

    let started = Instant::now();
    let outcome = client
        .get(format!("http://{addr}/spin"))
        .header("HOST", "spin-pooled")
        .send()
        .await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < SPIN_BOUND,
        "the spinning call must be ended by abandonment, took {elapsed:?}"
    );
    if let Ok(resp) = outcome {
        assert!(
            resp.status().is_server_error(),
            "a trapped call must fail, got {}",
            resp.status()
        );
    }

    // The spun instance had served 2 requests; had it survived, this would read
    // 3. A fresh instance reading 1 is the trap and reap, observed.
    let count = eventually(SPIN_BOUND, async || {
        served(&client, addr, "spin-pooled").await
    })
    .await?;
    assert_eq!(
        count, 1,
        "the request after the spin must land on a fresh instance"
    );
    Ok(())
}

/// A service whose call is abandoned is trapped like anything else, and its
/// supervisor restarts it: a restart beats carrying a wedged call — and the
/// whole singleton with it — for the rest of the workload's life. A spin that
/// finishes *inside* the deadline is never abandoned and so never touched,
/// however hot it ran.
///
/// The `bystander` workload shows the trap stayed in its own store. Two
/// workloads need the `DynamicRouter`; the `DevRouter` sends every request to
/// whichever workload resolved last.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spinning_service_is_trapped_and_restarted() -> Result<()> {
    let _serial = abandonment_env();
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    start_sleeper(&host, "spin-svc", true).await?;
    start_sleeper(&host, "bystander", false).await?;
    let client = client()?;

    assert_eq!(served(&client, addr, "spin-svc").await?, 1);
    assert_eq!(served(&client, addr, "bystander").await?, 1);

    // Hot but bounded: the spin replies inside the deadline, so nothing is
    // ever abandoned and the instance must survive it (`served` climbs on).
    let (status, body) = get(&client, addr, "spin-svc", "/spin?ms=500").await?;
    assert!(
        status.is_success(),
        "a spin that beats the deadline must not be touched, got {status}"
    );
    assert_eq!(
        json_u64_field(&body, "served"),
        2,
        "the same service instance must answer a bounded spin"
    );

    // Unbounded: the deadline passes, the call is abandoned, and the store is
    // trapped rather than carrying the wedged call forever.
    let started = Instant::now();
    let outcome = client
        .get(format!("http://{addr}/spin"))
        .header("HOST", "spin-svc")
        .send()
        .await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < SPIN_BOUND,
        "the spinning service call must be ended by abandonment, took {elapsed:?}"
    );
    if let Ok(resp) = outcome {
        assert!(
            resp.status().is_server_error(),
            "a trapped service call must fail, got {}",
            resp.status()
        );
    }

    // The supervisor restarts the service: a fresh instance answers, counting
    // from one again (the wedged instance was at 3).
    let count = eventually(SPIN_BOUND, async || served(&client, addr, "spin-svc").await).await?;
    assert_eq!(
        count, 1,
        "the restarted service must answer on a fresh instance"
    );

    // And the trap stayed in its own store: the neighbour counts on.
    assert_eq!(
        served(&client, addr, "bystander").await?,
        2,
        "a trapped service must not disturb another workload's instance"
    );
    Ok(())
}

/// The same failure, arrived at the way it actually happens: a component driven
/// into an unbounded loop by its *input*.
///
/// `/redos` matches a crafted string against `^(a+)+$` with the naive
/// backtracking a JS or PCRE engine uses — 2^n steps, ~10^15 at the default `n`.
/// Nothing about the component is looping on purpose, and the attacker controls
/// only a query parameter. It makes no host calls, so it never yields, so no
/// host-side timeout is ever polled: on a runtime without abandonment this
/// request never returns and its instance is lost for the life of the process.
///
/// This also covers the amplification. `max_concurrency` is 4, so `try_send`
/// admits three more requests onto the wedged instance and queues them for a run
/// loop that is stuck mid-poll and will never read them. Trapping the store is
/// what fails them; without it they hang until their clients give up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_component_wedged_by_its_input_is_trapped_and_replaced() -> Result<()> {
    let _serial = abandonment_env();
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    start_sleeper(&host, "redos", false).await?;
    let client = client()?;

    assert_eq!(served(&client, addr, "redos").await?, 1);

    // Three more alongside it, to fill `max_concurrency` on the same instance.
    let started = Instant::now();
    let mut inflight = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let (client, addr) = (client.clone(), addr);
        inflight.spawn(async move {
            client
                .get(format!("http://{addr}/redos"))
                .header("HOST", "redos")
                .send()
                .await
                .map(|r| r.status().as_u16())
        });
    }
    let outcomes = inflight.join_all().await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < SPIN_BOUND,
        "the wedged requests must be ended by abandonment, took {elapsed:?}"
    );
    // A transport error is a pass; the assertion is that the request failed.
    // Only a request that came back has a status to check.
    for status in outcomes.iter().flatten() {
        assert!(
            (500..600).contains(status),
            "a request wedged by its input must fail, got {status}"
        );
    }

    // The instance was reaped, so the component still serves: a fresh instance
    // answers, counting from one.
    let count = eventually(SPIN_BOUND, async || served(&client, addr, "redos").await).await?;
    assert_eq!(count, 1, "the component must recover on a fresh instance");
    Ok(())
}

/// A linked call served from a **cold ephemeral store** whose guest never
/// yields is ended by abandonment. The wedge is two components deep: HTTP
/// reaches the caller, the caller's imported `run(0)` wedges the callee, and
/// the callee's store — built for this one call — is the one that must die.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abandoned_linked_call_to_a_cold_ephemeral_store_is_ended() -> Result<()> {
    let _serial = abandonment_env();
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    start_linked(&host, "linked-cold", 0).await?;
    let client = client()?;

    let (status, body) = get(&client, addr, "linked-cold", "/").await?;
    assert!(status.is_success(), "warmup failed: {status}");
    assert_eq!(body, "43", "the linked call must round-trip");

    // The callee spins in its own store; the caller's dispatcher abandons the
    // call at the deadline and the epoch deadline ends the callee. The caller
    // sees its import fail, so this request errors rather than hanging.
    let started = Instant::now();
    let outcome = client
        .get(format!("http://{addr}/spin"))
        .header("HOST", "linked-cold")
        .send()
        .await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < SPIN_BOUND,
        "the wedged linked call must be ended by abandonment, took {elapsed:?}"
    );
    if let Ok(resp) = outcome {
        assert!(
            resp.status().is_server_error(),
            "a wedged linked call must fail, got {}",
            resp.status()
        );
    }

    // The workload recovers whole: caller and callee both serve again.
    let body = eventually(SPIN_BOUND, async || {
        let (status, body) = get(&client, addr, "linked-cold", "/").await?;
        anyhow::ensure!(status.is_success(), "status {status}");
        Ok(body)
    })
    .await?;
    assert_eq!(body, "43", "the linked path must recover after the trap");
    Ok(())
}

/// A linked call served from a **pooled warm instance** whose guest never
/// yields is ended by abandonment, and the pool replaces the instance. The
/// callee's `calls` counter lives in its instance's linear memory, so a count
/// that climbs and then restarts at one is the reap, observed from inside.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abandoned_linked_call_to_a_pooled_instance_is_ended() -> Result<()> {
    let _serial = abandonment_env();
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    start_linked(&host, "linked-pooled", 1).await?;
    let client = client()?;

    // Two calls land on the same warm callee instance: its count climbs.
    let (_, first) = get(&client, addr, "linked-pooled", "/calls").await?;
    let (_, second) = get(&client, addr, "linked-pooled", "/calls").await?;
    assert_eq!((first.as_str(), second.as_str()), ("1", "2"));

    let started = Instant::now();
    let outcome = client
        .get(format!("http://{addr}/spin"))
        .header("HOST", "linked-pooled")
        .send()
        .await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < SPIN_BOUND,
        "the wedged pooled linked call must be ended by abandonment, took {elapsed:?}"
    );
    if let Ok(resp) = outcome {
        assert!(
            resp.status().is_server_error(),
            "a wedged pooled linked call must fail, got {}",
            resp.status()
        );
    }

    // The wedged callee instance was reaped: a fresh one counts from one.
    let body = eventually(SPIN_BOUND, async || {
        let (status, body) = get(&client, addr, "linked-pooled", "/calls").await?;
        anyhow::ensure!(status.is_success(), "status {status}");
        Ok(body)
    })
    .await?;
    assert_eq!(
        body, "1",
        "the call after the spin must land on a fresh callee instance"
    );
    Ok(())
}

/// Deliver one message to a trigger service, the way a messaging backend does.
/// The outer `Result` is delivery (this is what abandonment bounds); the inner
/// one is the handler's own `result<_, string>`, which `msg-counter` uses to
/// echo its running count.
async fn deliver(
    ingress: &std::sync::Arc<
        wash_runtime::host::http::Ingress<wash_runtime::host::http::DevRouter>,
    >,
    workload_id: &str,
    subject: &str,
) -> Result<Result<(), String>> {
    use wash_runtime::host::http::HostHandler as _;
    ingress
        .deliver_trigger_service_message(
            workload_id,
            wash_runtime::host::trigger_service::BrokerMessage {
                subject: subject.to_string(),
                body: b"hi".to_vec(),
                reply_to: None,
            },
        )
        .await
}

/// A message delivery wedged in a non-yielding handler is ended by
/// abandonment: the delivery errors at the deadline instead of hanging, and
/// the service supervisor restarts the instance (its count resets).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abandoned_message_delivery_traps_and_restarts_the_service() -> Result<()> {
    let _serial = abandonment_env();
    let (_addr, host, ingress) = common::start_host_with_p3_handler("127.0.0.1:0").await?;
    let workload_id = uuid::Uuid::new_v4().to_string();
    host.workload_start(WorkloadStartRequest {
        workload_id: workload_id.clone(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "msg-spin".to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(MSG_COUNTER_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 2,
            }),
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    })
    .await
    .context("msg-counter service should start")?;

    // The handler echoes its running count as the error string: delivery works
    // and this instance has served one message.
    assert_eq!(
        deliver(&ingress, &workload_id, "first").await?,
        Err("other: 1:first".to_string())
    );

    // `spin` wedges the handler. The delivery must error at the deadline
    // rather than hang — on a runtime without abandonment this await never
    // returns.
    let started = Instant::now();
    let outcome = deliver(&ingress, &workload_id, "spin").await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < SPIN_BOUND,
        "the wedged delivery must be ended by abandonment, took {elapsed:?}"
    );
    assert!(
        outcome.is_err(),
        "a wedged delivery must error, got {outcome:?}"
    );

    // The supervisor restarts the service: a fresh instance counts from one.
    let echoed = eventually(SPIN_BOUND, async || {
        deliver(&ingress, &workload_id, "after")
            .await?
            .err()
            .context("handler echoes via the err arm")
    })
    .await?;
    assert_eq!(
        echoed, "other: 1:after",
        "the restarted service must count from one"
    );
    Ok(())
}

/// A **healthy** handler that outruns its delivery deadline must keep its
/// service, however long it goes on.
///
/// The counterweight to the test above. What separates the two is whether the
/// guest yields.
///
/// The `chatter` subject spends all its time awaiting, in hops short enough that
/// each wakes onto an expired epoch deadline, so its fires land exactly as a
/// pinned guest's do and bank the same credit. It is also the only call on the
/// store, so the wanted-call gate does not apply. `watch_until_abandoned`
/// deregisters it because the handler yields; the `spin` subject can never let
/// that happen.
///
/// A delivery runs unbounded from inside the store, on the service singleton
/// every other subject shares, so without deregistration a slow handler stays
/// visible to the epoch callback for as long as it runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chatty_messaging_handler_survives_being_abandoned() -> Result<()> {
    let _serial = abandonment_env();
    let (_addr, host, ingress) = common::start_host_with_p3_handler("127.0.0.1:0").await?;
    let workload_id = uuid::Uuid::new_v4().to_string();
    host.workload_start(WorkloadStartRequest {
        workload_id: workload_id.clone(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "msg-chatter".to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(MSG_COUNTER_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 2,
            }),
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    })
    .await
    .context("msg-counter service should start")?;

    assert_eq!(
        deliver(&ingress, &workload_id, "first").await?,
        Err("other: 1:first".to_string())
    );

    // Abandoned at its deadline: the handler is slower than any caller will
    // wait, which is not itself an offence. All that matters here is that the
    // delivery does not succeed.
    let outcome = deliver(&ingress, &workload_id, "chatter").await;
    assert!(
        !matches!(outcome, Ok(Ok(()))),
        "a delivery slower than its deadline must not report success, got {outcome:?}"
    );

    // Stay quiet while the handler hops: a delivery in this window would re-arm
    // the epoch deadline and shield the hops from firing at all, and an
    // otherwise-idle store is precisely the scenario. `CHATTER` covers the
    // fixture's hops with room to spare.
    // The fixture hops for ~6.4s (16 x 400ms), and a trap needs only the
    // deadline plus the grace, so waiting ~2x the hop budget leaves the count
    // below unambiguous.
    //
    // One delivery and one exact assertion rather than a poll: every delivery
    // increments the counter, so a retry loop would walk a restarted handler up
    // to the expected count and pass on the failure it exists to catch.
    tokio::time::sleep(Duration::from_secs(12)).await;

    // Untouched, the handler counted its own delivery, making this the third on
    // the same instance.
    let echoed = deliver(&ingress, &workload_id, "after")
        .await?
        .err()
        .context("handler echoes via the err arm")?;
    anyhow::ensure!(
        echoed != "other: 1:after",
        "a healthy chatty handler was trapped over its abandoned delivery: the \
         service restarted and is counting from one"
    );
    anyhow::ensure!(
        echoed == "other: 3:after",
        "expected the third delivery on an undisturbed instance, got {echoed}; \
         if this is `2:after` the chatter had not finished yet and the wait \
         above is too short for this runner"
    );
    Ok(())
}

/// A capability call wedged in a host component plugin is warned about at the
/// grace and **escalated to a trap**: a non-yielding activation holds the
/// store's guest execution, so no other tenant's call can enter and the
/// singleton is already down for everyone. Trapping it at the escalation is
/// what brings it back — the supervisor rebuilds the store and replays binds,
/// the same path an organic plugin trap takes. The in-memory state dies with
/// the store; that is the documented blast radius of the shared singleton,
/// and per-task cancellation (bytecodealliance/wasmtime#11833) is what would
/// shrink it to the one bad call.
#[cfg(feature = "host-component-plugins")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abandoned_capability_call_escalates_to_a_plugin_restart() -> Result<()> {
    use common::{
        component_workload_request, kv_plugin_caller_host_interfaces,
        start_host_with_component_plugin,
    };

    let _serial = abandonment_env();
    let (addr, host) =
        start_host_with_component_plugin("127.0.0.1:0", "kv-plugin", KV_PLUGIN_WASM).await?;
    host.workload_start(component_workload_request(
        "kv-caller",
        "kv-caller",
        KV_PLUGIN_CALLER_WASM,
        LocalResources::default(),
        kv_plugin_caller_host_interfaces("kv-caller"),
    ))
    .await
    .context("kv caller workload should start")?;
    let client = client()?;

    // Seed state in the plugin's singleton store.
    let (status, _) = get(&client, addr, "kv-caller", "/set?key=a&value=before").await?;
    assert!(status.is_success(), "seed set failed: {status}");

    // `__spin__` wedges the plugin-side call. The caller's request must fail
    // at its deadline rather than hang.
    let started = Instant::now();
    let outcome = client
        .get(format!("http://{addr}/get?key=__spin__"))
        .header("HOST", "kv-caller")
        .send()
        .await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < SPIN_BOUND,
        "the wedged capability call must be abandoned, took {elapsed:?}"
    );
    if let Ok(resp) = outcome {
        assert!(
            resp.status().is_server_error(),
            "a wedged capability call must fail, got {}",
            resp.status()
        );
    }

    // The wedged plugin is trapped at the escalation and rebuilt under
    // supervision: service comes back for everyone.
    let body = eventually(SPIN_BOUND, async || {
        let (status, body) = get(&client, addr, "kv-caller", "/set?key=b&value=after").await?;
        anyhow::ensure!(status.is_success(), "set: status {status}, body: {body}");
        let (status, body) = get(&client, addr, "kv-caller", "/get?key=b").await?;
        anyhow::ensure!(status.is_success(), "get: status {status}, body: {body}");
        Ok(body)
    })
    .await?;
    assert_eq!(body, "after", "the rebuilt plugin must serve again");

    // And it really is a fresh incarnation: the state seeded before the wedge
    // died with the trapped store.
    let (status, _) = get(&client, addr, "kv-caller", "/get?key=a").await?;
    assert_eq!(
        status.as_u16(),
        404,
        "the seeded state must be gone after the supervised restart"
    );
    Ok(())
}

/// A healthy plugin activation that outruns its deadline must not cost every
/// tenant the shared store.
///
/// `__chatter__` is as slow as `__spin__` and just as invisible to the sampling:
/// same fire pattern, same execution credit, and the only call on the store, so
/// the wanted-call gate does not apply. It survives because it yields, which
/// lets `watch_until_abandoned` deregister it a grace after the dispatcher gives
/// up.
///
/// The seeded key is the assertion. It lives in the plugin's in-memory store, so
/// it survives only if there was no supervised restart.
#[cfg(feature = "host-component-plugins")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chatty_capability_call_does_not_restart_the_plugin() -> Result<()> {
    use common::{
        component_workload_request, kv_plugin_caller_host_interfaces,
        start_host_with_component_plugin,
    };

    let _serial = abandonment_env();
    let (addr, host) =
        start_host_with_component_plugin("127.0.0.1:0", "kv-plugin", KV_PLUGIN_WASM).await?;
    host.workload_start(component_workload_request(
        "kv-caller",
        "kv-caller",
        KV_PLUGIN_CALLER_WASM,
        LocalResources::default(),
        kv_plugin_caller_host_interfaces("kv-caller"),
    ))
    .await
    .context("kv caller workload should start")?;
    let client = client()?;

    // State in the plugin's singleton store, which a restart would destroy.
    let (status, _) = get(&client, addr, "kv-caller", "/set?key=a&value=before").await?;
    assert!(status.is_success(), "seed set failed: {status}");

    // The caller gives up at its deadline. Being slower than any caller will
    // wait is not itself an offence.
    let outcome = client
        .get(format!("http://{addr}/get?key=__chatter__"))
        .header("HOST", "kv-caller")
        .send()
        .await;
    if let Ok(resp) = outcome {
        assert!(
            !resp.status().is_success(),
            "a call slower than its deadline must not report success, got {}",
            resp.status()
        );
    }

    // The fixture hops for ~12s, outlasting the deadline (2s), grace (1s) and
    // escalation (3s) combined, so a trap has every opportunity to land. Wait
    // it out so the plugin is idle again before asking.
    tokio::time::sleep(Duration::from_secs(16)).await;

    // Untouched: the same incarnation still serves, and still remembers.
    let (status, body) = get(&client, addr, "kv-caller", "/get?key=a").await?;
    anyhow::ensure!(
        status.is_success() && body == "before",
        "a healthy chatty activation restarted the shared plugin: /get?key=a returned \
         {status} {body:?}, wanted 200 \"before\"; the seeded state only survives if the \
         store was never trapped"
    );

    // Still usable for new work, not merely alive.
    let (status, _) = get(&client, addr, "kv-caller", "/set?key=b&value=after").await?;
    assert!(status.is_success(), "post-chatter set failed: {status}");
    let (status, body) = get(&client, addr, "kv-caller", "/get?key=b").await?;
    assert!(status.is_success(), "post-chatter get failed: {status}");
    assert_eq!(body, "after", "the plugin must keep serving new calls");
    Ok(())
}

/// A client that gives up must cost a healthy call nothing, even when the guest
/// keeps working long past both the deadline and the grace.
///
/// A disconnect arms the flag, so acting on abandonment alone would condemn the
/// store a grace later and take every co-tenant request with it. The guest here
/// yields throughout, so its stretch keeps resetting and the store is left
/// alone. The bystander requests are the assertion: same instance throughout,
/// `served` climbing unbroken rather than restarting at 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_gives_up_does_not_disturb_a_healthy_slow_guest() -> Result<()> {
    let _serial = abandonment_env();
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    start_sleeper(&host, "slow-svc", true).await?;
    let client = client()?;

    assert_eq!(served(&client, addr, "slow-svc").await?, 1);

    // A request the guest spends far longer on than deadline + grace, whose
    // client walks away almost at once — dropping the future is a disconnect.
    let abandon_ms = (DEADLINE_SECS + GRACE_SECS) * 1_000 + 4_000;
    let mut gave_up = Box::pin(
        client
            .get(format!("http://{addr}/slow?ms={abandon_ms}"))
            .header("HOST", "slow-svc")
            .send(),
    );
    tokio::select! {
        _ = &mut gave_up => anyhow::bail!("the slow request answered before the client gave up"),
        () = tokio::time::sleep(Duration::from_millis(300)) => {}
    }
    drop(gave_up);

    // Well past deadline + grace, the same instance must still be serving.
    let until = Instant::now() + Duration::from_millis(abandon_ms);
    let mut previous = 1;
    while Instant::now() < until {
        let now = served(&client, addr, "slow-svc")
            .await
            .context("a healthy service must keep serving after a client disconnects")?;
        anyhow::ensure!(
            now > previous,
            "the service was restarted by an abandoned-but-healthy call: served {previous} -> {now}"
        );
        previous = now;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    anyhow::ensure!(
        previous > 5,
        "expected the bystander traffic to keep flowing, only saw {previous}"
    );
    Ok(())
}

/// The streaming form of the same guarantee: an SSE / long-poll client that
/// disconnects between heartbeats.
///
/// The head arrives at once, so the call rides on the body wrapper, which the
/// disconnect drops and arms. The guest only finds out at its next frame write,
/// so with a heartbeat longer than the grace it is always still holding the
/// call when the grace expires — and only its pauses between frames distinguish
/// it from a wedged guest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_sse_client_disconnecting_between_frames_does_not_trap_the_service() -> Result<()> {
    let _serial = abandonment_env();
    // Frames further apart than deadline + grace, so the guest is mid-sleep
    // for the whole window in which a time-only trap would fire.
    const GAP_MS: u64 = (DEADLINE_SECS + GRACE_SECS) * 1_000 + 2_000;

    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    start_sleeper(&host, "sse-svc", true).await?;
    let client = client()?;

    assert_eq!(served(&client, addr, "sse-svc").await?, 1);

    // Take the head, then walk away without reading the body: a closed tab.
    let stream = client
        .get(format!("http://{addr}/sse?frames=10&gap_ms={GAP_MS}"))
        .header("HOST", "sse-svc")
        .send()
        .await
        .context("SSE response head should arrive immediately")?;
    assert!(
        stream.status().is_success(),
        "SSE head should be 2xx, got {}",
        stream.status()
    );
    drop(stream);

    // Past deadline + grace with the guest asleep between frames.
    let until = Instant::now() + Duration::from_millis(GAP_MS);
    let mut previous = 1;
    while Instant::now() < until {
        let now = served(&client, addr, "sse-svc")
            .await
            .context("a service must survive an SSE client disconnecting")?;
        anyhow::ensure!(
            now > previous,
            "the service was restarted by an abandoned SSE stream: served {previous} -> {now}"
        );
        previous = now;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    anyhow::ensure!(previous > 5, "expected steady traffic, only saw {previous}");
    Ok(())
}

/// A store with a call somebody still wants must not be trapped over a call
/// somebody else gave up on.
///
/// The chatty stream here wakes every 150ms — closer together than the pause
/// threshold, so its epoch fires chain into what continuity reads as one
/// pinned stretch. When a co-tenant's client walks away and that call passes
/// its grace, the first two trap conditions hold and the third looks like it
/// does; the stream's own still-wanted call is what must keep the store
/// alive. The frames all arriving is the assertion — a trap cuts the stream
/// mid-flight and restarts the service.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abandoned_co_tenant_does_not_trap_a_chatty_healthy_stream() -> Result<()> {
    let _serial = abandonment_env();
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    start_sleeper(&host, "chatty-svc", true).await?;
    let client = client()?;

    assert_eq!(served(&client, addr, "chatty-svc").await?, 1);

    // The co-tenant: a slow request whose client walks away, arming its flag
    // at ~0.3s; it stays registered until its exchange bound ends its task.
    let mut gone = Box::pin(
        client
            .get(format!("http://{addr}/slow?ms=8000"))
            .header("HOST", "chatty-svc")
            .send(),
    );
    tokio::select! {
        _ = &mut gone => anyhow::bail!("the slow request answered before the client gave up"),
        () = tokio::time::sleep(Duration::from_millis(300)) => {}
    }
    drop(gone);

    // The wanted call: sub-threshold frames for 1.5s, spanning the whole
    // window in which the abandoned co-tenant is past its grace.
    let resp = client
        .get(format!("http://{addr}/sse?frames=10&gap_ms=150"))
        .header("HOST", "chatty-svc")
        .send()
        .await
        .context("SSE response head should arrive immediately")?;
    anyhow::ensure!(resp.status().is_success(), "sse head: {}", resp.status());
    let body = resp
        .bytes()
        .await
        .context("the chatty stream must survive an abandoned co-tenant")?;
    let frames = String::from_utf8_lossy(&body).matches("data: ").count();
    anyhow::ensure!(frames == 10, "stream cut short: {frames}/10 frames");

    // And the same instance is still serving: a trap would have restarted it.
    let mut previous = 1;
    for _ in 0..3 {
        let now = served(&client, addr, "chatty-svc")
            .await
            .context("the service must keep serving after the stream ends")?;
        anyhow::ensure!(
            now > previous,
            "the service was restarted: served {previous} -> {now}"
        );
        previous = now;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    Ok(())
}

/// A guest that awaits repeatedly in **short hops** must survive being
/// abandoned, exactly like the single-long-await guest above.
///
/// Each hop wakes onto an expired epoch deadline, so the store's fires land
/// one hop apart — a wake pattern that wall-clock sampling cannot tell from a
/// pinned guest inside any single window.
///
/// The guest yields, so `watch_until_abandoned` deregisters the call a grace
/// after the client disconnects, and a store with nothing registered is never
/// acted on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_awaiting_in_short_hops_survives_being_abandoned() -> Result<()> {
    let _serial = abandonment_env();
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    start_sleeper(&host, "hoppy-svc", true).await?;
    let client = client()?;

    assert_eq!(served(&client, addr, "hoppy-svc").await?, 1);

    // Eight 400ms hops ≈ 3.2s of constant short awaits, whose client walks
    // away almost at once: the call is abandoned past its grace while the
    // guest is still hopping, with no other call registered to shield it.
    let mut gone = Box::pin(
        client
            .get(format!("http://{addr}/chatter?hops=8&hop_ms=400"))
            .header("HOST", "hoppy-svc")
            .send(),
    );
    tokio::select! {
        _ = &mut gone => anyhow::bail!("the chatter answered before the client gave up"),
        () = tokio::time::sleep(Duration::from_millis(300)) => {}
    }
    drop(gone);

    // Stay quiet while the guest hops: a request in this window would re-arm
    // the epoch deadline and shield the hops from firing at all, and the
    // scenario is precisely an otherwise-idle store. Then the same instance
    // must still be serving, its count climbing from where it left off.
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    let mut previous = 1;
    for _ in 0..4 {
        let now = served(&client, addr, "hoppy-svc")
            .await
            .context("a hopping guest must not be trapped when abandoned")?;
        anyhow::ensure!(
            now > previous,
            "the service was restarted over a short-hop awaiter: served {previous} -> {now}"
        );
        previous = now;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Ok(())
}

/// A busy but *healthy* instance must never be touched, however long it lives.
///
/// There is no CPU budget and no heuristic left to get wrong here: a store is
/// only ever acted on when a call on it is abandoned, and steady successful
/// traffic abandons nothing. Driving many deadline-periods' worth of requests
/// is what pins that down — each does ~5ms of guest work and yields, and the
/// instance must survive with its `served` count climbing unbroken; a trap
/// would show up as a failed request and a count restarting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_healthy_instance_under_steady_traffic_is_never_trapped() -> Result<()> {
    let _serial = abandonment_env();
    // Several deadline periods, so a spurious once-per-period trap cannot hide.
    const DRIVE: Duration = Duration::from_secs(DEADLINE_SECS * 3);
    const GAP: Duration = Duration::from_millis(200);

    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    start_sleeper(&host, "healthy", false).await?;
    let client = client()?;

    let started = Instant::now();
    let mut previous = 0;
    while started.elapsed() < DRIVE {
        let served = served(&client, addr, "healthy")
            .await
            .with_context(|| format!("request failed at t={:?}", started.elapsed()))?;
        anyhow::ensure!(
            served > previous,
            "the instance was replaced at t={:?}: served went {previous} -> {served}",
            started.elapsed()
        );
        previous = served;
        tokio::time::sleep(GAP).await;
    }
    anyhow::ensure!(previous > 5, "expected steady traffic, only saw {previous}");
    Ok(())
}

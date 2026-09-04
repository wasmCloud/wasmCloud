//! The host's probe listener: liveness and readiness, on a port of their own.
//!
//! A `tcpSocket` probe against the traffic port asks whether there is room in
//! the listen backlog, which the kernel answers whether or not the process is
//! running. This listener answers from what the host knows about itself.
//!
//! `/livez` is "restart me" and fails only when the command loop has stopped —
//! the cure costs every workload on the host. `/readyz` is "stop sending me
//! work" and fails while starting, while draining, and while the ingress is at
//! its ceiling, which a TCP probe cannot express at all.

use core::time::Duration;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use anyhow::Context as _;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};

use crate::host::accept::AcceptBackoff;

/// Paths this listener answers. Anything else is a 404, so a misconfigured
/// probe fails loudly rather than passing against the wrong endpoint.
const LIVEZ: &str = "/livez";
const READYZ: &str = "/readyz";

/// Connections this listener will hold at once.
///
/// It needs a ceiling because its port is reachable from the whole cluster and
/// spends the same descriptors the ingress ceiling is a share of. It needs a
/// generous one, and connections that end quickly, because shedding here costs
/// the kubelet its probe and the host a restart — see [`CONNECTION_TIMEOUT`].
const MAX_PROBE_CONNECTIONS: usize = 512;

/// How long one probe connection may live. Each serves a single request and
/// closes, so holding a slot open is not something a caller has a reason to do.
///
/// The only deadline here, and it wraps the whole connection: a peer that opens
/// a socket and says nothing loses its slot on this alone.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// A monotonic "still turning" signal, beaten by whatever owns the host's main
/// loop.
///
/// Held as milliseconds since this was built rather than as an `Instant`, so a
/// beat is one relaxed store on a loop that runs for the life of the host.
/// [`NEVER_BEATEN`] separates "has not started" from "has stopped": startup is
/// readiness's to report, and restarting a host for being slow to start is how
/// a slow start becomes a crash loop.
#[derive(Debug)]
pub struct Liveness {
    start: Instant,
    last_beat_ms: AtomicU64,
    max_silence: Duration,
}

/// `last_beat_ms` before the first beat. No real elapsed-millis reading can
/// collide with it.
const NEVER_BEATEN: u64 = u64::MAX;

impl Liveness {
    /// A signal that goes stale after `max_silence` without a beat.
    ///
    /// Size it well above whatever paces the loop being watched: the point is
    /// to catch a loop that has stopped, not one that is between ticks.
    pub fn new(max_silence: Duration) -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            last_beat_ms: AtomicU64::new(NEVER_BEATEN),
            max_silence,
        })
    }

    /// Record that the loop turned.
    pub fn beat(&self) {
        let elapsed = self
            .start
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        self.last_beat_ms.store(elapsed, Ordering::Relaxed);
    }

    /// How long since the last beat, or `None` before the first.
    pub fn silence(&self) -> Option<Duration> {
        match self.last_beat_ms.load(Ordering::Relaxed) {
            NEVER_BEATEN => None,
            last => Some(
                self.start
                    .elapsed()
                    .saturating_sub(Duration::from_millis(last)),
            ),
        }
    }

    fn alive(&self) -> bool {
        self.silence()
            .is_none_or(|silence| silence <= self.max_silence)
    }
}

/// Something the host can be not-ready because of.
///
/// Readiness is a claim about whether more work should be sent here, so every
/// implementor answers for one reason it should not be.
pub trait ReadinessCheck: Send + Sync + std::fmt::Debug {
    /// Named in the `/readyz` body, so an operator reading a probe failure
    /// learns which condition failed without reaching for the logs.
    fn name(&self) -> &'static str;

    /// `true` when this check is satisfied.
    fn ready(&self) -> bool;

    /// Whether a refusal from this check is one the host cannot recover from
    /// on its own, so `/livez` should fail with it and the host be restarted.
    ///
    /// Almost nothing qualifies, and the default says so: failing liveness
    /// takes every workload on the host and hands the scheduler all of them at
    /// once, which is far worse than a host that is briefly not taking work.
    /// It earns its place only where readiness alone leaves a host that will
    /// never serve again — running, heartbeating, and still being scheduled
    /// onto, because nothing that places work reads readiness.
    fn terminal(&self) -> bool {
        false
    }
}

/// The state [`serve`] reports, and the handle a host drives it through.
#[derive(Clone, Debug, Default)]
pub struct ProbeState {
    liveness: Option<Arc<Liveness>>,
    started: Arc<AtomicBool>,
    draining: Arc<AtomicBool>,
    checks: Vec<Arc<dyn ReadinessCheck>>,
}

impl ProbeState {
    /// Watch `liveness` for `/livez`. Without one, `/livez` reports alive
    /// whenever the listener can answer at all — still stronger than a TCP
    /// probe, which the kernel answers whether or not the runtime is running.
    #[must_use]
    pub fn with_liveness(mut self, liveness: Arc<Liveness>) -> Self {
        self.liveness = Some(liveness);
        self
    }

    /// Add a reason the host may be not-ready.
    #[must_use]
    pub fn with_readiness(mut self, check: Arc<dyn ReadinessCheck>) -> Self {
        self.checks.push(check);
        self
    }

    /// Report ready-if-nothing-else-objects from now on.
    ///
    /// Until this is called `/readyz` refuses, because the listener binds before
    /// the host is subscribed to its command subject and before any workload is
    /// placed. Reporting ready in that window puts the pod in the Service with
    /// no routes behind it.
    pub fn started(&self) {
        self.started.store(true, Ordering::Relaxed);
    }

    /// Report not-ready from now on, without touching liveness.
    ///
    /// Called when the host starts draining: the endpoint should leave the
    /// Service while in-flight requests finish, and a host that answered
    /// `/livez` with a failure instead would be killed mid-drain.
    pub fn drain(&self) {
        self.draining.store(true, Ordering::Relaxed);
    }

    fn live(&self) -> bool {
        if !self.liveness.as_ref().is_none_or(|l| l.alive()) {
            return false;
        }
        // A draining host stops accepting because it was told to. Reading that
        // as "restart me" would kill it mid-drain, which is the one thing
        // `drain` exists to avoid.
        if self.draining.load(Ordering::Relaxed) {
            return true;
        }
        !self
            .checks
            .iter()
            .any(|check| check.terminal() && !check.ready())
    }

    /// The checks currently refusing, empty when the host is ready.
    fn not_ready(&self) -> Vec<&'static str> {
        if self.draining.load(Ordering::Relaxed) {
            return vec!["draining"];
        }
        if !self.started.load(Ordering::Relaxed) {
            return vec!["starting"];
        }
        self.checks
            .iter()
            .filter(|check| !check.ready())
            .map(|check| check.name())
            .collect()
    }
}

fn text(status: StatusCode, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(body.into()));
    *response.status_mut() = status;
    response
}

fn answer(state: &ProbeState, req: &Request<hyper::body::Incoming>) -> Response<Full<Bytes>> {
    match req.uri().path() {
        LIVEZ if state.live() => text(StatusCode::OK, "ok\n"),
        LIVEZ => text(StatusCode::SERVICE_UNAVAILABLE, "stalled\n"),
        READYZ => match state.not_ready() {
            reasons if reasons.is_empty() => text(StatusCode::OK, "ok\n"),
            reasons => text(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("{}\n", reasons.join(",")),
            ),
        },
        _ => text(StatusCode::NOT_FOUND, "not found\n"),
    }
}

/// Bind the probe listener, before the host takes work.
///
/// Separate from [`serve`] because a port already taken leaves every probe
/// failing, and that has to fail the command rather than a background task.
pub async fn bind(addr: SocketAddr) -> anyhow::Result<TcpListener> {
    TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind the probe listener on {addr}"))
}

/// Serve an already-bound probe listener until `shutdown` resolves.
///
/// Its own accept loop rather than a route on the ingress, so saturating the
/// data plane cannot starve the answer to "is this host alive", and paced by the
/// same [`AcceptBackoff`]: whatever exhausts the process's descriptors exhausts
/// them for both listeners.
pub async fn serve(
    listener: TcpListener,
    state: ProbeState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) {
    if let Ok(bound) = listener.local_addr() {
        info!(addr = %bound, "probe server listening");
    }

    let connections = Arc::new(tokio::sync::Semaphore::new(MAX_PROBE_CONNECTIONS));
    let mut backoff = AcceptBackoff::default();
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        let pause = backoff.pause();
        tokio::select! {
            () = &mut shutdown => {
                info!("probe server received shutdown signal");
                return;
            }
            accepted = async {
                if let Some(pause) = pause {
                    tokio::time::sleep(pause).await;
                }
                listener.accept().await
            } => {
                let (client, peer) = match accepted {
                    Ok(accepted) => accepted,
                    Err(e) => {
                        if backoff.failed(&e) {
                            error!(err = ?e, failures = backoff.failures(), "probe server cannot accept connections");
                        } else {
                            debug!(err = ?e, "failed to accept a probe connection");
                        }
                        continue;
                    }
                };
                let Ok(slot) = Arc::clone(&connections).try_acquire_owned() else {
                    debug!(addr = ?peer, "probe listener at its ceiling; closing");
                    drop(client);
                    continue;
                };
                let state = state.clone();
                tokio::spawn(async move {
                    let _slot = slot;
                    let service = service_fn(move |req| {
                        let response = answer(&state, &req);
                        async move { Ok::<_, std::convert::Infallible>(response) }
                    });
                    let mut builder = hyper::server::conn::http1::Builder::new();
                    builder
                        .timer(TokioTimer::new())
                        // One request per connection: a slot held open is a
                        // slot the kubelet cannot have. `CONNECTION_TIMEOUT`
                        // below is the only deadline; see its comment.
                        .keep_alive(false);
                    let served = tokio::time::timeout(
                        CONNECTION_TIMEOUT,
                        builder.serve_connection(TokioIo::new(client), service),
                    );
                    match served.await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => warn!(addr = ?peer, err = ?e, "error serving a probe request"),
                        Err(_) => debug!(addr = ?peer, "probe connection timed out"),
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Gate {
        name: &'static str,
        ready: AtomicBool,
        terminal: bool,
    }

    impl ReadinessCheck for Gate {
        fn name(&self) -> &'static str {
            self.name
        }
        fn ready(&self) -> bool {
            self.ready.load(Ordering::Relaxed)
        }
        fn terminal(&self) -> bool {
            self.terminal
        }
    }

    fn gate(name: &'static str, ready: bool) -> Arc<Gate> {
        Arc::new(Gate {
            name,
            ready: AtomicBool::new(ready),
            terminal: false,
        })
    }

    /// A check whose refusal the host cannot recover from.
    fn terminal_gate(name: &'static str, ready: bool) -> Arc<Gate> {
        Arc::new(Gate {
            name,
            ready: AtomicBool::new(ready),
            terminal: true,
        })
    }

    /// The distinction the TCP probe cannot make: a host at its ceiling is
    /// still alive — restarting it would lose every workload on it — but it
    /// must stop being sent work, or the Service keeps routing to a host that
    /// sheds everything it is offered.
    #[test]
    fn a_saturated_host_is_live_but_not_ready() {
        let saturated = gate("http_ingress_saturated", false);
        let state = ProbeState::default().with_readiness(saturated);
        state.started();

        assert!(state.live(), "saturation is not a reason to restart");
        assert_eq!(state.not_ready(), vec!["http_ingress_saturated"]);
    }

    /// The other side of that distinction. An accept loop that has ended is
    /// not coming back, and nothing that places work reads readiness — the
    /// scheduler goes by the Host CR's heartbeat, which this host still sends.
    /// Left to readiness alone it keeps taking workloads it can never serve, so
    /// this is the case where a restart is the lesser loss.
    #[test]
    fn a_host_whose_ingress_stopped_is_not_live() {
        let stopped = terminal_gate("http_ingress_stopped", false);
        let state = ProbeState::default().with_readiness(stopped);
        state.started();

        assert!(!state.live(), "an ingress that cannot recover must restart");
        assert_eq!(state.not_ready(), vec!["http_ingress_stopped"]);
    }

    /// The same check, satisfied, says nothing about liveness.
    #[test]
    fn a_terminal_check_that_is_satisfied_leaves_the_host_live() {
        let state = ProbeState::default().with_readiness(terminal_gate("ingress", true));
        state.started();
        assert!(state.live());
        assert!(state.not_ready().is_empty());
    }

    /// A host on its way out stops accepting because it was told to. Reading
    /// that as "restart me" would kill it mid-drain, which is the one thing
    /// draining exists to avoid.
    #[test]
    fn a_draining_host_stays_live_even_with_its_ingress_stopped() {
        let state =
            ProbeState::default().with_readiness(terminal_gate("http_ingress_stopped", false));
        state.started();
        state.drain();

        assert!(state.live(), "a draining host must not be restarted");
        assert_eq!(state.not_ready(), vec!["draining"]);
    }

    /// Draining is the same shape and the more common one: leave the Service,
    /// keep serving what is in flight, and do not get killed doing it.
    #[test]
    fn draining_is_not_ready_but_stays_live() {
        let state = ProbeState::default().with_readiness(gate("ingress", true));
        state.started();
        assert!(state.not_ready().is_empty());

        state.drain();
        assert_eq!(state.not_ready(), vec!["draining"]);
        assert!(state.live(), "a draining host must not be restarted");
    }

    /// Every refusing check is named, so a probe failure says which without
    /// needing the logs — and draining answers alone, since once the host is
    /// going away the rest is noise.
    #[test]
    fn readiness_names_what_is_refusing() {
        let state = ProbeState::default()
            .with_readiness(gate("first", false))
            .with_readiness(gate("second", true))
            .with_readiness(gate("third", false));
        state.started();
        assert_eq!(state.not_ready(), vec!["first", "third"]);

        state.drain();
        assert_eq!(state.not_ready(), vec!["draining"]);
    }

    /// The listener binds before the host has a NATS subscription or a single
    /// workload, so until the host says it started, readiness has to refuse.
    /// A pod that joins the Service in that window is sent traffic that nothing
    /// is behind.
    #[test]
    fn a_host_that_has_not_started_is_not_ready() {
        let state = ProbeState::default().with_readiness(gate("ingress", true));
        assert_eq!(state.not_ready(), vec!["starting"]);
        assert!(state.live(), "starting is not a reason to restart");

        state.started();
        assert!(state.not_ready().is_empty());
    }

    /// The whole point of the probe listener over a `tcpSocket` probe:
    /// answering needs the runtime to schedule a task, not just the kernel to
    /// complete a handshake. Driven over a real socket so the routing, the
    /// status codes and the 404 for a mistyped path are all pinned.
    #[tokio::test]
    async fn the_listener_answers_both_paths_over_a_socket() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let saturated = gate("http_ingress_saturated", true);
        let state =
            ProbeState::default().with_readiness(Arc::clone(&saturated) as Arc<dyn ReadinessCheck>);
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();

        // Port 0, so this test does not fight whatever else is running.
        let listener = bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let served = state.clone();
        served.started();
        tokio::spawn(async move {
            let _ = serve(listener, served, async {
                let _ = stopped.await;
            })
            .await;
        });

        async fn get(addr: SocketAddr, path: &str) -> String {
            for _ in 0..50 {
                let Ok(mut conn) = tokio::net::TcpStream::connect(addr).await else {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                };
                let request =
                    format!("GET {path} HTTP/1.1\r\nHost: probes\r\nConnection: close\r\n\r\n");
                conn.write_all(request.as_bytes()).await.unwrap();
                let mut body = String::new();
                conn.read_to_string(&mut body).await.unwrap();
                return body;
            }
            panic!("probe server never came up on {addr}");
        }

        assert!(get(addr, LIVEZ).await.starts_with("HTTP/1.1 200"));
        assert!(get(addr, READYZ).await.starts_with("HTTP/1.1 200"));

        // A full ingress: still alive, no longer ready.
        saturated.ready.store(false, Ordering::Relaxed);
        assert!(get(addr, LIVEZ).await.starts_with("HTTP/1.1 200"));
        let ready = get(addr, READYZ).await;
        assert!(ready.starts_with("HTTP/1.1 503"), "{ready}");
        assert!(ready.contains("http_ingress_saturated"), "{ready}");

        // A mistyped probe path must fail rather than pass against nothing.
        assert!(get(addr, "/healthz").await.starts_with("HTTP/1.1 404"));

        let _ = stop.send(());
    }

    /// A loop that stopped turning is what `/livez` exists to catch, and the
    /// only thing it should: a host with no liveness source registered reports
    /// alive whenever it can answer.
    #[test]
    fn liveness_goes_stale_without_a_beat() {
        let liveness = Liveness::new(Duration::from_millis(50));
        let state = ProbeState::default().with_liveness(Arc::clone(&liveness));
        assert!(state.live(), "a fresh signal starts alive");

        // Still starting, not yet stalled: a host slow to bind plugins and pull
        // images has not failed, and restarting it for that is how a slow start
        // becomes a crash loop.
        std::thread::sleep(Duration::from_millis(120));
        assert!(
            state.live(),
            "a loop that has not started yet is not stalled"
        );

        liveness.beat();
        std::thread::sleep(Duration::from_millis(120));
        assert!(!state.live(), "silence past the bound is a stalled host");

        liveness.beat();
        assert!(state.live(), "a beat brings it back");

        assert!(
            ProbeState::default().live(),
            "with nothing to watch, answering at all is the signal"
        );
    }
}

//! Gated throughput measurement for the shared-Postgres-pool template.
//!
//! Drives closed-loop HTTP load at increasing concurrency against the same
//! workload `integration_postgres_e2e` asserts on, and reports where the chain
//! saturates. Three layers are measured separately so the cost can be
//! attributed:
//!
//!  * `/` — the Service's router alone (no backend call, no database).
//!  * `/users` — the full chain: router -> linked backend call -> sqlx ->
//!    loopback pool -> upstream session -> Postgres.
//!  * a direct `tokio-postgres` client from the test process — the same
//!    `SELECT`, with none of the workload in the way.
//!
//! Postgres-side counters are sampled around each phase, so the per-request
//! upstream cost (sessions dialed, transactions executed) is measured from the
//! server rather than inferred.
//!
//! Requires Docker and `wash`. Marked `#[ignore]` so the default suite never
//! pays the cost.
//!
//! Run with:
//!   cargo test --test integration_postgres_bench -- --ignored --nocapture

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{CmdWaitFor, ExecCommand, IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::time::{Instant, timeout};

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3_http_handler};

const HOST: &str = "pg-bench";

/// Concurrency levels swept per phase.
const LEVELS: &[usize] = &[1, 2, 4, 8, 16, 32];

/// Wall-clock spent measuring at each concurrency level.
const PHASE: Duration = Duration::from_secs(6);

/// Requests issued (and discarded) before each phase's measurement window.
const WARMUP: usize = 20;

const INIT_SQL: &str = "\
CREATE TABLE IF NOT EXISTS users (id SERIAL PRIMARY KEY, name TEXT NOT NULL, email TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS todos (id SERIAL PRIMARY KEY, title TEXT NOT NULL, done BOOLEAN NOT NULL DEFAULT FALSE);
INSERT INTO users (name, email) VALUES
    ('Ada Lovelace', 'ada@example.com'),
    ('Alan Turing', 'alan@example.com'),
    ('Grace Hopper', 'grace@example.com');
INSERT INTO todos (title, done) VALUES
    ('Write a wasi:http service', TRUE),
    ('Pool connections to Postgres', TRUE),
    ('Ship the template', FALSE);";

fn template_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/http-sqlx-postgres")
}

fn wash_binary() -> PathBuf {
    if let Some(wash) = std::env::var_os("WASH").filter(|s| !s.is_empty()) {
        return PathBuf::from(wash);
    }
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target_dir = workspace.join(
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target")),
    );
    for profile in ["release", "debug"] {
        let candidate = target_dir.join(profile).join("wash");
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from("wash")
}

fn primary_non_loopback_ip() -> Result<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    sock.connect("8.8.8.8:80")?;
    let ip = sock.local_addr()?.ip();
    anyhow::ensure!(!ip.is_loopback(), "resolved a loopback IP ({ip})");
    Ok(ip)
}

fn build_template() -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let dir = template_dir();
    let rel = dir.join("target/wasm32-wasip2/release");
    let paths = [
        rel.join("service.wasm"),
        rel.join("users.wasm"),
        rel.join("todos.wasm"),
    ];
    if !paths.iter().all(|p| p.exists()) {
        let wash = wash_binary();
        eprintln!("building templates/http-sqlx-postgres with `{} build`…", wash.display());
        let status = Command::new(&wash)
            .arg("build")
            .current_dir(&dir)
            .status()
            .with_context(|| format!("failed to run `{} build`", wash.display()))?;
        anyhow::ensure!(status.success(), "`wash build` failed for http-sqlx-postgres");
    }
    Ok((
        std::fs::read(&paths[0]).context("read service.wasm")?,
        std::fs::read(&paths[1]).context("read users.wasm")?,
        std::fs::read(&paths[2]).context("read todos.wasm")?,
    ))
}

fn local_resources(env: HashMap<String, String>) -> LocalResources {
    LocalResources {
        environment: env,
        allowed_hosts: vec!["*".parse().unwrap()].into(),
        ..Default::default()
    }
}

/// `warm` is the per-backend `pool_size`. Every warm instance holds its sqlx
/// connection — and therefore pins one upstream session — for as long as it
/// lives, so the total across both backends must stay below the service's
/// `MAX_SESSIONS` or a burst served by cold instances would wait on a session
/// that is never returned.
fn workload(
    service: Vec<u8>,
    users: Vec<u8>,
    todos: Vec<u8>,
    upstream: &str,
    warm: i32,
) -> WorkloadStartRequest {
    let mut svc_env = HashMap::new();
    svc_env.insert("UPSTREAM_ADDR".to_string(), upstream.to_string());
    svc_env.insert("UPSTREAM_USER".to_string(), "app".to_string());
    svc_env.insert("UPSTREAM_PASSWORD".to_string(), "app".to_string());
    svc_env.insert("UPSTREAM_DB".to_string(), "app".to_string());

    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "bench".to_string(),
            name: HOST.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: service.into(),
                local_resources: local_resources(svc_env),
                max_restarts: 0,
            }),
            components: vec![
                Component {
                    name: "users".to_string(),
                    digest: None,
                    bytes: users.into(),
                    local_resources: local_resources(HashMap::new()),
                    pool_size: warm,
                    max_invocations: 0,
                },
                Component {
                    name: "todos".to_string(),
                    digest: None,
                    bytes: todos.into(),
                    local_resources: local_resources(HashMap::new()),
                    pool_size: warm,
                    max_invocations: 0,
                },
            ],
            host_interfaces: http_only_host_interfaces(HOST),
            volumes: vec![],
        },
    }
}

async fn get_with_retry(
    client: &reqwest::Client,
    addr: &std::net::SocketAddr,
    path: &str,
) -> Result<String> {
    let deadline = Duration::from_secs(60);
    let start = Instant::now();
    let mut last = String::new();
    while start.elapsed() < deadline {
        match timeout(
            Duration::from_secs(15),
            client
                .get(format!("http://{addr}{path}"))
                .header("HOST", HOST)
                .send(),
        )
        .await
        {
            Ok(Ok(resp)) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    return Ok(body);
                }
                last = format!("status {status}: {body}");
            }
            Ok(Err(e)) => last = format!("send error: {e}"),
            Err(_) => last = "request timed out".to_string(),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    anyhow::bail!("{path} never succeeded within {deadline:?}; last: {last}")
}

async fn psql_i64(postgres: &ContainerAsync<GenericImage>, sql: &str) -> Result<i64> {
    let mut res = postgres
        .exec(
            ExecCommand::new(["psql", "-U", "app", "-d", "app", "-t", "-A", "-c", sql])
                .with_cmd_ready_condition(CmdWaitFor::exit()),
        )
        .await
        .map_err(|e| anyhow::anyhow!("psql exec failed: {e}"))?;
    let out = res.stdout_to_vec().await.context("psql stdout")?;
    anyhow::ensure!(
        matches!(res.exit_code().await, Ok(Some(0))),
        "psql query failed: {sql}"
    );
    String::from_utf8_lossy(&out)
        .trim()
        .parse::<i64>()
        .with_context(|| format!("unexpected psql output for {sql}"))
}

/// Server-side counters for the `app` database, sampled around a phase.
#[derive(Clone, Copy, Debug)]
struct PgCounters {
    /// Cumulative sessions ever established.
    sessions: i64,
    /// Cumulative committed transactions (a simple `Query` counts as one).
    xacts: i64,
    /// Sessions currently open over TCP (the pool's upstream connections).
    open: i64,
}

async fn pg_counters(postgres: &ContainerAsync<GenericImage>) -> Result<PgCounters> {
    Ok(PgCounters {
        sessions: psql_i64(
            postgres,
            "SELECT sessions FROM pg_stat_database WHERE datname = 'app'",
        )
        .await?,
        xacts: psql_i64(
            postgres,
            "SELECT xact_commit + xact_rollback FROM pg_stat_database WHERE datname = 'app'",
        )
        .await?,
        open: psql_i64(
            postgres,
            "SELECT count(*) FROM pg_stat_activity WHERE usename = 'app' AND client_addr IS NOT NULL",
        )
        .await?,
    })
}

/// Latency summary for one measurement window.
struct Stats {
    completed: u64,
    errors: u64,
    elapsed: Duration,
    /// Sorted per-request latencies, in microseconds.
    latencies: Vec<u64>,
}

impl Stats {
    fn rps(&self) -> f64 {
        self.completed as f64 / self.elapsed.as_secs_f64()
    }
    fn pct(&self, p: f64) -> f64 {
        if self.latencies.is_empty() {
            return f64::NAN;
        }
        let idx = ((self.latencies.len() as f64 - 1.0) * p).round() as usize;
        self.latencies
            .get(idx)
            .map(|us| *us as f64 / 1000.0)
            .unwrap_or(f64::NAN)
    }
    fn mean_ms(&self) -> f64 {
        if self.latencies.is_empty() {
            return f64::NAN;
        }
        self.latencies.iter().sum::<u64>() as f64 / self.latencies.len() as f64 / 1000.0
    }
}

/// Median of a sorted microsecond sample, in milliseconds.
fn median_ms(sorted: &[u64]) -> f64 {
    sorted
        .get(sorted.len() / 2)
        .map(|us| *us as f64 / 1000.0)
        .unwrap_or(f64::NAN)
}

/// Closed-loop load: `concurrency` workers each issue requests back-to-back for
/// `PHASE`, recording per-request latency.
async fn load(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    path: &'static str,
    concurrency: usize,
) -> Stats {
    // Warm the path (and, for /users, let the pool finish prewarming) before
    // the window opens.
    for _ in 0..WARMUP {
        let _ = client
            .get(format!("http://{addr}{path}"))
            .header("HOST", HOST)
            .send()
            .await;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let errors = Arc::new(AtomicU64::new(0));
    let mut workers = Vec::new();
    let started = Instant::now();
    for _ in 0..concurrency {
        let client = client.clone();
        let stop = Arc::clone(&stop);
        let errors = Arc::clone(&errors);
        workers.push(tokio::spawn(async move {
            let mut latencies: Vec<u64> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let t0 = Instant::now();
                let res = client
                    .get(format!("http://{addr}{path}"))
                    .header("HOST", HOST)
                    .send()
                    .await;
                let ok = match res {
                    Ok(resp) => {
                        let status = resp.status();
                        // Drain the body: the response streams out of the
                        // guest, so time-to-last-byte is the real cost.
                        let body_ok = resp.bytes().await.is_ok();
                        status.is_success() && body_ok
                    }
                    Err(_) => false,
                };
                if ok {
                    latencies.push(t0.elapsed().as_micros() as u64);
                } else {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
            }
            latencies
        }));
    }
    tokio::time::sleep(PHASE).await;
    stop.store(true, Ordering::Relaxed);

    let mut latencies = Vec::new();
    for w in workers {
        latencies.extend(w.await.unwrap_or_default());
    }
    let elapsed = started.elapsed();
    latencies.sort_unstable();
    Stats {
        completed: latencies.len() as u64,
        errors: errors.load(Ordering::Relaxed),
        elapsed,
        latencies,
    }
}

fn header(title: &str) {
    println!("\n=== {title} ===");
    println!(
        "{:>5}  {:>9}  {:>9}  {:>9}  {:>9}  {:>9}  {:>7}",
        "conc", "req/s", "mean ms", "p50 ms", "p95 ms", "p99 ms", "errors"
    );
}

fn row(concurrency: usize, s: &Stats) {
    println!(
        "{:>5}  {:>9.1}  {:>9.2}  {:>9.2}  {:>9.2}  {:>9.2}  {:>7}",
        concurrency,
        s.rps(),
        s.mean_ms(),
        s.pct(0.50),
        s.pct(0.95),
        s.pct(0.99),
        s.errors
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Docker + wash; run with --ignored"]
async fn bench_postgres_shared_pool() -> Result<()> {
    let (service_wasm, users_wasm, todos_wasm) = build_template()?;

    let postgres = GenericImage::new("postgres", "17-alpine")
        .with_exposed_port(5432.tcp())
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_USER", "app")
        .with_env_var("POSTGRES_PASSWORD", "app")
        .with_env_var("POSTGRES_DB", "app")
        .with_env_var("POSTGRES_HOST_AUTH_METHOD", "password")
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start Postgres container: {e}"))?;

    let pg_port = postgres
        .get_host_port_ipv4(5432)
        .await
        .map_err(|e| anyhow::anyhow!("failed to get Postgres host port: {e}"))?;
    let host_ip = primary_non_loopback_ip().context("no non-loopback host IP available")?;
    let upstream = format!("{host_ip}:{pg_port}");

    let mut seeded = false;
    for _ in 0..40 {
        let mut res = postgres
            .exec(
                ExecCommand::new([
                    "psql", "-U", "app", "-d", "app", "-v", "ON_ERROR_STOP=1", "-c", INIT_SQL,
                ])
                .with_cmd_ready_condition(CmdWaitFor::exit()),
            )
            .await
            .map_err(|e| anyhow::anyhow!("psql exec failed: {e}"))?;
        let _ = res.stdout_to_vec().await;
        if matches!(res.exit_code().await, Ok(Some(0))) {
            seeded = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    anyhow::ensure!(seeded, "failed to seed schema into Postgres");
    std::net::TcpStream::connect(&upstream)
        .with_context(|| format!("host cannot reach published Postgres at {upstream}"))?;

    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    let warm: i32 = std::env::var("BENCH_WARM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    println!("\nbackend pool_size (warm instances each): {warm}");
    host.workload_start(workload(
        service_wasm,
        users_wasm,
        todos_wasm,
        &upstream,
        warm,
    ))
        .await
        .context("failed to start the shared-pool workload")?;

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .timeout(Duration::from_secs(30))
        .build()?;

    // Prove the chain works before measuring it.
    let body = get_with_retry(&client, &addr, "/users").await?;
    anyhow::ensure!(body.contains("Ada Lovelace"), "unexpected /users body: {body}");
    println!("\n/users body: {body}");

    // Phase 1: router only. No backend call, no database — the floor for
    // anything this workload can serve.
    header("/ (router only: HTTP ingress -> service, no backend, no DB)");
    for &c in LEVELS {
        let s = load(&client, addr, "/", c).await;
        row(c, &s);
    }

    // Phase 2: the full chain, with Postgres-side counters around each level.
    header("/users (router -> linked backend -> sqlx -> pool -> Postgres)");
    let mut per_request_cost: Vec<(usize, f64, f64, i64)> = Vec::new();
    for &c in LEVELS {
        let before = pg_counters(&postgres).await?;
        let s = load(&client, addr, "/users", c).await;
        let after = pg_counters(&postgres).await?;
        row(c, &s);
        let n = s.completed.max(1) as f64;
        per_request_cost.push((
            c,
            (after.sessions - before.sessions) as f64 / n,
            (after.xacts - before.xacts) as f64 / n,
            after.open,
        ));
    }

    println!("\n--- upstream cost per request (measured server-side) ---");
    println!(
        "{:>5}  {:>16}  {:>16}  {:>12}",
        "conc", "sessions/req", "xacts/req", "open conns"
    );
    for (c, sessions, xacts, open) in &per_request_cost {
        println!("{c:>5}  {sessions:>16.3}  {xacts:>16.3}  {open:>12}");
    }

    // Phase 3: the same SELECT straight from the test process over one
    // connection, serialized — the database's own floor for this query.
    let (pg_client, connection) = tokio_postgres::connect(
        &format!("host={host_ip} port={pg_port} user=app password=app dbname=app"),
        tokio_postgres::NoTls,
    )
    .await
    .context("direct connect to Postgres")?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let stmt = pg_client
        .prepare("SELECT id, name, email FROM users ORDER BY id")
        .await?;
    for _ in 0..WARMUP {
        let _ = pg_client.query(&stmt, &[]).await?;
    }
    let mut direct: Vec<u64> = Vec::new();
    let t0 = Instant::now();
    while t0.elapsed() < PHASE {
        let q0 = Instant::now();
        let _ = pg_client.query(&stmt, &[]).await?;
        direct.push(q0.elapsed().as_micros() as u64);
    }
    let elapsed = t0.elapsed();
    direct.sort_unstable();
    let direct = Stats {
        completed: direct.len() as u64,
        errors: 0,
        elapsed,
        latencies: direct,
    };
    header("direct tokio-postgres (same SELECT, no workload in the path)");
    row(1, &direct);

    // The reset tax. Every client disconnect makes the pool run
    // `Sync` + `ROLLBACK` + `DISCARD ALL` on the upstream before the session is
    // reusable, so measure what those two extra statements cost on the same
    // connection — that is the floor for the reset, independent of the guest.
    // Measured on their own: `DISCARD ALL` deallocates prepared statements, so
    // pairing them with the prepared `SELECT` above would invalidate it.
    let mut reset: Vec<u64> = Vec::new();
    let t0 = Instant::now();
    while t0.elapsed() < PHASE {
        let q0 = Instant::now();
        pg_client.batch_execute("ROLLBACK").await?;
        pg_client.batch_execute("DISCARD ALL").await?;
        reset.push(q0.elapsed().as_micros() as u64);
    }
    let elapsed = t0.elapsed();
    reset.sort_unstable();
    let reset = Stats {
        completed: reset.len() as u64,
        errors: 0,
        elapsed,
        latencies: reset,
    };
    header("direct: ROLLBACK + DISCARD ALL alone (the pool's per-disconnect reset)");
    row(1, &reset);
    println!(
        "reset costs {:.2} ms on top of a {:.2} ms SELECT",
        reset.pct(0.50),
        direct.pct(0.50)
    );
    // `DISCARD ALL` dropped it; the scaling phase below needs it back.
    let stmt = pg_client
        .prepare("SELECT id, name, email FROM users ORDER BY id")
        .await?;

    // Phase 4: how the chain scales with result-set size. The pool splices the
    // upstream at *message* granularity, and Postgres sends one `DataRow` per
    // row, so this is where a per-message cost would show up. Measured against
    // the direct client over the same rows to separate proxy cost from
    // database cost. Run last: it grows the `users` table.
    println!("\n=== result-set scaling (concurrency 1) ===");
    println!(
        "{:>8}  {:>12}  {:>12}  {:>12}  {:>10}",
        "rows", "/users ms", "direct ms", "proxy ms", "us/row"
    );
    let mut prev: Option<(i64, f64)> = None;
    for rows in [3i64, 100, 1_000, 10_000] {
        let mut res = postgres
            .exec(
                ExecCommand::new([
                    "psql",
                    "-U",
                    "app",
                    "-d",
                    "app",
                    "-v",
                    "ON_ERROR_STOP=1",
                    "-c",
                    &format!(
                        "TRUNCATE users; INSERT INTO users (name, email) \
                         SELECT 'user ' || g, 'user' || g || '@example.com' \
                         FROM generate_series(1, {rows}) g;"
                    ),
                ])
                .with_cmd_ready_condition(CmdWaitFor::exit()),
            )
            .await
            .map_err(|e| anyhow::anyhow!("psql exec failed: {e}"))?;
        let _ = res.stdout_to_vec().await;
        anyhow::ensure!(
            matches!(res.exit_code().await, Ok(Some(0))),
            "failed to resize users to {rows} rows"
        );

        // Serial samples through the whole chain.
        let mut through: Vec<u64> = Vec::new();
        for i in 0..30 {
            let t0 = Instant::now();
            let resp = client
                .get(format!("http://{addr}/users"))
                .header("HOST", HOST)
                .send()
                .await?;
            anyhow::ensure!(resp.status().is_success(), "/users failed at {rows} rows");
            let body = resp.bytes().await?;
            if i >= 10 {
                through.push(t0.elapsed().as_micros() as u64);
            }
            if i == 0 {
                anyhow::ensure!(
                    body.len() > 10,
                    "unexpectedly small body at {rows} rows: {}",
                    String::from_utf8_lossy(&body)
                );
            }
        }
        through.sort_unstable();
        let through_ms = median_ms(&through);

        // The same query, direct.
        let mut d: Vec<u64> = Vec::new();
        for i in 0..30 {
            let q0 = Instant::now();
            let _ = pg_client.query(&stmt, &[]).await?;
            if i >= 10 {
                d.push(q0.elapsed().as_micros() as u64);
            }
        }
        d.sort_unstable();
        let direct_ms = median_ms(&d);

        let proxy_ms = through_ms - direct_ms;
        let per_row = match prev {
            Some((prev_rows, prev_proxy)) if rows > prev_rows => {
                format!(
                    "{:.1}",
                    (proxy_ms - prev_proxy) * 1000.0 / (rows - prev_rows) as f64
                )
            }
            _ => "-".to_string(),
        };
        prev = Some((rows, proxy_ms));
        println!("{rows:>8}  {through_ms:>12.2}  {direct_ms:>12.2}  {proxy_ms:>12.2}  {per_row:>10}");
    }

    drop(host);
    Ok(())
}

//! Gated end-to-end test for the shared-Postgres-pool template against a real
//! Postgres.
//!
//! Exercises every layer at once: a long-lived p3 Service that is both the
//! HTTP ingress router (`wasi:http/handler`) and the shared session pool
//! (`wasi:cli/run` listening on `127.0.0.1:6432`), routing inbound HTTP over
//! the cross-store bridge to the stateless `users`/`todos` backends, which run
//! ordinary sqlx through the Service's pool of pre-authenticated sessions to
//! the upstream Postgres.
//!
//! The backends run with a deliberately low `max_invocations` so instances are
//! recycled mid-test — the serverless-churn case the pool exists for. Two
//! assertions then hold the template to its claims:
//!
//!  * concurrent server connections stay bounded by the pool cap, and
//!  * the *cumulative* session count stays flat, proving upstream sessions
//!    were reused across requests and instance churn rather than re-dialed.
//!
//! Requires Docker (Postgres via testcontainers) and `wash` to build the
//! `templates/http-sqlx-postgres` component sources. Marked `#[ignore]` so the
//! default suite never pays the cost.
//!
//! Run with:
//!   cargo test --test integration_postgres_e2e -- --ignored --nocapture

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashMap, path::PathBuf, process::Command, time::Duration};

use anyhow::{Context, Result};
use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{CmdWaitFor, ExecCommand, IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::time::timeout;

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3_http_handler};

const HOST: &str = "pg-e2e";

/// The template's pool cap (`MAX_SESSIONS` in `service/src/lib.rs`).
const POOL_CAP: i64 = 4;

/// Schema + seed rows the backends read. Inlined (rather than `include_str!`d
/// from the template) so this test compiles even where the template is absent;
/// the template is only needed at runtime, when the gate is enabled. Mirrors
/// `templates/http-sqlx-postgres/db/init.sql`.
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

/// The wash binary to build the template with: `WASH` env override, then the
/// workspace build (honoring `CARGO_TARGET_DIR`, like xtask's `ensure_wash`),
/// then `PATH`. In CI the `cargo build` step has already produced
/// `target/debug/wash`; nothing installs wash on `PATH` there.
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

/// The host's primary non-loopback IPv4, discovered via the local address a UDP
/// socket picks when "connecting" outward (no packets are sent).
fn primary_non_loopback_ip() -> Result<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    sock.connect("8.8.8.8:80")?;
    let ip = sock.local_addr()?.ip();
    anyhow::ensure!(!ip.is_loopback(), "resolved a loopback IP ({ip})");
    Ok(ip)
}

/// Build the template components with `wash build` (only when gated) and return
/// their wasm bytes: (service, users, todos).
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
        eprintln!(
            "building templates/http-sqlx-postgres with `{} build`…",
            wash.display()
        );
        // The template's build.command shells out to `wash` again (per-crate
        // `wash -C <crate> wit fetch`), so the resolved binary's directory
        // must be on the child's PATH — CI has no wash installed.
        let orig_path = std::env::var_os("PATH").unwrap_or_default();
        let path = match wash
            .canonicalize()
            .ok()
            .and_then(|w| w.parent().map(PathBuf::from))
        {
            Some(wash_dir) => std::env::join_paths(
                std::iter::once(wash_dir).chain(std::env::split_paths(&orig_path)),
            )
            .context("failed to join PATH entries")?,
            None => orig_path,
        };
        let status = Command::new(&wash)
            .arg("build")
            .env("PATH", &path)
            .current_dir(&dir)
            .status()
            .with_context(|| format!("failed to run `{} build`", wash.display()))?;
        anyhow::ensure!(
            status.success(),
            "`wash build` failed for http-sqlx-postgres"
        );
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
        // The Service dials the upstream Postgres and the backends dial the
        // loopback pool over wasi:sockets; allow all egress.
        allowed_hosts: vec!["*".parse().unwrap()].into(),
        ..Default::default()
    }
}

fn workload(
    service: Vec<u8>,
    users: Vec<u8>,
    todos: Vec<u8>,
    upstream: &str,
) -> WorkloadStartRequest {
    let mut svc_env = HashMap::new();
    svc_env.insert("UPSTREAM_ADDR".to_string(), upstream.to_string());
    // The credentials the Service authenticates its pooled sessions with. Note
    // that the stateless backends receive no environment at all.
    svc_env.insert("UPSTREAM_USER".to_string(), "app".to_string());
    svc_env.insert("UPSTREAM_PASSWORD".to_string(), "app".to_string());
    svc_env.insert("UPSTREAM_DB".to_string(), "app".to_string());

    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
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
                    pool_size: 1,
                    // Deliberately low: recycles backend instances mid-test to
                    // exercise the pool's reset-and-reuse path under the
                    // serverless churn it exists for.
                    max_invocations: 5,
                },
                Component {
                    name: "todos".to_string(),
                    digest: None,
                    bytes: todos.into(),
                    local_resources: local_resources(HashMap::new()),
                    pool_size: 1,
                    max_invocations: 5,
                },
            ],
            host_interfaces: http_only_host_interfaces(HOST),
            volumes: vec![],
        },
    }
}

/// GET `path`, returning the body once it succeeds. Retries while the
/// service's pool is still coming up (cli/run binds `:6432` asynchronously and
/// the upstream may still be restarting after init).
async fn get_with_retry(
    client: &reqwest::Client,
    addr: &std::net::SocketAddr,
    path: &str,
) -> Result<String> {
    let deadline = Duration::from_secs(45);
    let start = tokio::time::Instant::now();
    let mut last = String::new();
    while start.elapsed() < deadline {
        match timeout(
            Duration::from_secs(10),
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

/// Run a single-value query inside the container via psql (local socket, so it
/// does not disturb the TCP connection stats it is used to read).
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

/// Server connections currently open over TCP as the app user (the pool's
/// sessions; local-socket psql sessions have a NULL client_addr).
async fn open_tcp_sessions(postgres: &ContainerAsync<GenericImage>) -> Result<i64> {
    psql_i64(
        postgres,
        "SELECT count(*) FROM pg_stat_activity WHERE usename = 'app' AND client_addr IS NOT NULL",
    )
    .await
}

/// Cumulative sessions ever established against the app database.
async fn total_sessions(postgres: &ContainerAsync<GenericImage>) -> Result<i64> {
    psql_i64(
        postgres,
        "SELECT sessions FROM pg_stat_database WHERE datname = 'app'",
    )
    .await
}

#[tokio::test]
#[ignore = "requires Docker + wash; run with --ignored"]
async fn test_postgres_shared_pool_e2e() -> Result<()> {
    // Build the template components (sqlx backends + pool service).
    let (service_wasm, users_wasm, todos_wasm) = build_template()?;

    // Start a real Postgres with password auth, mirroring the template's
    // docker-compose.yml: the pool must actually authenticate its sessions.
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
    // The runtime virtualizes 127.0.0.1 as an in-process loopback (guest
    // listeners only), so the guest cannot reach Docker's published port there.
    // Use the host's non-loopback IP, which routes through real OS sockets to
    // the container's published port (Docker publishes on 0.0.0.0).
    let host_ip = primary_non_loopback_ip().context("no non-loopback host IP available")?;
    let upstream = format!("{host_ip}:{pg_port}");

    // Seed the schema. Postgres restarts once after first-time init, so retry
    // the exec until it lands on the ready server.
    let mut seeded = false;
    for _ in 0..40 {
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
                    INIT_SQL,
                ])
                .with_cmd_ready_condition(CmdWaitFor::exit()),
            )
            .await
            .map_err(|e| anyhow::anyhow!("psql exec failed: {e}"))?;
        let _ = res.stdout_to_vec().await; // drain (blocks until exit)
        if matches!(res.exit_code().await, Ok(Some(0))) {
            seeded = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    anyhow::ensure!(seeded, "failed to seed schema into Postgres");

    // Preflight: the guest reaches the upstream over real OS sockets, so the
    // host must too. Fail fast with a clear message rather than via a slow retry.
    std::net::TcpStream::connect(&upstream)
        .with_context(|| format!("host cannot reach published Postgres at {upstream}"))?;

    // Baseline for the reuse assertion, taken before the workload exists.
    let sessions_before = total_sessions(&postgres).await?;

    // Start the host and the shared-pool workload.
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    host.workload_start(workload(service_wasm, users_wasm, todos_wasm, &upstream))
        .await
        .context("failed to start the shared-pool workload")?;

    let client = reqwest::Client::new();

    // /users: router -> bridge -> stateless users backend -> sqlx -> loopback
    // pool -> pre-authenticated upstream session -> seeded rows back out.
    let users_body = get_with_retry(&client, &addr, "/users").await?;
    for name in ["Ada Lovelace", "Alan Turing", "Grace Hopper"] {
        assert!(
            users_body.contains(name),
            "/users should return seeded user {name:?}; got {users_body}"
        );
    }

    // /todos: same chain through the second backend.
    let todos_body = get_with_retry(&client, &addr, "/todos").await?;
    for title in [
        "Write a wasi:http service",
        "Pool connections to Postgres",
        "Ship the template",
    ] {
        assert!(
            todos_body.contains(title),
            "/todos should return seeded todo {title:?}; got {todos_body}"
        );
    }

    // A burst of requests across both backends. With max_invocations: 5 the
    // backend instances are recycled several times during this loop, dropping
    // their loopback connections without a Terminate — the pool must reset and
    // reuse the underlying sessions.
    let mut handles = Vec::new();
    for i in 0..16 {
        let client = client.clone();
        let path = if i % 2 == 0 { "/users" } else { "/todos" };
        handles.push(tokio::spawn(async move {
            get_with_retry(&client, &addr, path)
                .await
                .map(|body| (path, body))
        }));
    }
    for h in handles {
        let (path, body) = h.await.context("request task panicked")??;
        let expected = if path == "/users" {
            "Ada Lovelace"
        } else {
            "Write a wasi:http service"
        };
        assert!(
            body.contains(expected),
            "every concurrent {path} must return seeded rows; got {body}"
        );
    }

    // The template's claims, measured from the server side:
    //
    // 1. Concurrent connections stay bounded by the pool cap regardless of
    //    request count.
    let open = open_tcp_sessions(&postgres).await?;
    assert!(
        (1..=POOL_CAP).contains(&open),
        "expected 1..={POOL_CAP} pooled server connections, found {open}"
    );

    // 2. Sessions were REUSED, not re-dialed: 18 requests with several backend
    //    instance recyclings must establish only a handful of sessions (the
    //    prewarmed ones plus at most a dial per pool slot). A pool that
    //    silently reconnects per request or per recycled instance would blow
    //    well past this.
    let sessions_used = total_sessions(&postgres).await? - sessions_before;
    assert!(
        (1..=2 * POOL_CAP).contains(&sessions_used),
        "expected at most {} sessions for the whole run (reuse across churn), found {sessions_used}",
        2 * POOL_CAP
    );

    // 3. Upstream failure recovery: kill every pooled session server-side, as
    //    a database restart or failover would. (An actual container restart
    //    would re-randomize the published host port out from under the
    //    workload's fixed UPSTREAM_ADDR, testing the wrong thing.) Both
    //    pinned and idle sessions die at once; the pool must surface prompt
    //    errors (not hangs) to the clients, close the dead sessions to free
    //    their capacity slots, and dial fresh ones for the retries — requests
    //    must succeed again without restarting the workload. (get_with_retry
    //    absorbs the error window.)
    let killed = psql_i64(
        &postgres,
        "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity \
         WHERE usename = 'app' AND client_addr IS NOT NULL AND pid <> pg_backend_pid()",
    )
    .await?;
    assert!(
        killed >= 1,
        "expected pooled sessions to terminate, killed {killed}"
    );

    let users_body = get_with_retry(&client, &addr, "/users")
        .await
        .context("requests must recover after all pooled sessions are killed")?;
    assert!(
        users_body.contains("Ada Lovelace"),
        "post-kill /users should return seeded rows; got {users_body}"
    );
    let todos_body = get_with_retry(&client, &addr, "/todos").await?;
    assert!(
        todos_body.contains("Ship the template"),
        "post-kill /todos should return seeded rows; got {todos_body}"
    );

    // And the pool is once again bounded, with freshly-dialed sessions.
    let open = open_tcp_sessions(&postgres).await?;
    assert!(
        (1..=POOL_CAP).contains(&open),
        "expected 1..={POOL_CAP} pooled connections after recovery, found {open}"
    );

    Ok(())
}

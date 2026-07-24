# http-sqlx-postgres — serverless components, pooled Postgres

Serverless compute and database connection pooling pull in opposite
directions: stateless components can be torn down at any moment, but a
connection pool is only useful if it outlives requests. This template resolves
that by splitting the two across a workload:

- **`users`** and **`todos`** are stateless, serverless components. They run
  completely ordinary **sqlx** with a client-side `PgPool` — and hold **no
  database credentials**.
- **`service`** is the workload's one long-lived component. It owns the
  **shared connection pool**: a bounded set of sessions to the real Postgres,
  authenticated once with the one set of credentials and **reused across both
  backends and across instance churn** (session pooling, as in pgbouncer).

```
  GET /users   ┌───────────────────────── service ─────────────────────────┐
  GET /todos   │ wasi:http/handler ── route by path                        │
 ────────────► │      │ host-linked WIT calls                              │
               │      ▼                                                    │
               │  users / todos (stateless, serverless, sqlx PgPool)       │
               │      │ loopback 127.0.0.1:6432 (in-process, no creds)     │
               │      ▼                                                    │
               │ wasi:cli/run ── SHARED POOL of pre-authenticated sessions │
               └──────┼────────────────────────────────────────────────────┘
                      │ wasi:sockets + credentials (held ONLY here)
                      ▼
                  Postgres
```

## Why this shape

- **Serverless components can't pool.** A stateless instance's in-memory
  `PgPool` dies with it; per-request TCP + auth to Postgres is the classic
  serverless anti-pattern (and what managed poolers like RDS Proxy exist to
  fix). Here the pool lives in the one component whose lifecycle matches it.
- **The loopback hop is nearly free.** The runtime virtualizes `127.0.0.1`
  inside the workload, so a "connection" from `users` to the pool is an
  in-process byte stream — no real TCP, no TLS, no password. sqlx neither
  knows nor cares: it speaks normal Postgres protocol to `127.0.0.1:6432`.
- **Credentials stay in one place.** Only the `service` holds the database
  user/password; it pre-authenticates each pooled session. The stateless
  components' DSN has no secret in it.
- **Both backends share the same pool.** `users` and `todos` queries are
  served over the same bounded set of upstream sessions. When a client
  disconnects — cleanly or because a serverless instance was torn down — the
  session is reset (`DISCARD ALL`) and handed to the next client.

## Prerequisites

- A `wash` built with WASI 0.3 support (the `wasip3` feature).
- The `wasm32-wasip2` Rust target: `rustup target add wasm32-wasip2`.
- Docker (for the local Postgres).

## Run it

1. **Start Postgres** (seeds `users` and `todos` from `db/init.sql`):

   ```sh
   docker compose up -d
   ```

2. **Point the Service at your host.** The runtime routes `127.0.0.1` /
   `localhost` to the workload's *own* loopback network, not your machine, so
   the Service must reach Postgres at a **non-loopback, host-reachable
   address**. Find your host's LAN IP and set it in `.wash/config.yaml`:

   ```sh
   ipconfig getifaddr en0     # macOS; use `hostname -I` on Linux
   ```

   ```yaml
   # .wash/config.yaml
   workload:
     environment:
       config:
         UPSTREAM_ADDR: "<YOUR_HOST_IP>:5432"
   ```

3. **Run:**

   ```sh
   wash dev
   ```

4. **Call it:**

   ```sh
   curl http://127.0.0.1:8000/users
   curl http://127.0.0.1:8000/todos
   ```

5. **Watch the pool do its job.** Hammer the endpoints, then count the real
   connections Postgres sees — bounded by the pool cap (4), not by the number
   of requests:

   ```sh
   hey -n 500 http://127.0.0.1:8000/users   # or a curl loop
   docker compose exec postgres \
     psql -U app -c "SELECT count(*) FROM pg_stat_activity WHERE usename = 'app'"
   ```

## How it works

- **`service/src/lib.rs`** exports both `wasi:http/handler` (ingress: routes
  `/users` and `/todos` to the backends over host-linked WIT calls) and
  `wasi:cli/run` (the pool). The pool speaks enough Postgres wire protocol to
  own the handshakes on both sides: it answers the loopback client's startup
  itself (`AuthenticationOk` — the loopback is only reachable from inside the
  workload) and replays the real server's parameters, then splices the client
  onto a checked-out upstream session. Sessions are dialed and authenticated
  (cleartext password, matching `POSTGRES_HOST_AUTH_METHOD=password` in
  `docker-compose.yml`) up to a cap of 4, with 2 pre-warmed at startup;
  checkouts past the cap wait for a return.
- **`users/src/lib.rs`** / **`todos/src/lib.rs`** are plain sqlx: a
  `PgPool` against `postgres://app@127.0.0.1:6432/app` (note: no password),
  queried under `block_on` on a current-thread Tokio runtime (the sqlx
  `wasm32-wasip2` pattern). The client pool keeps the loopback connection warm
  while an instance lives; the Service's pool makes even a cold instance's
  first query hit a warm, already-authenticated database session.
- **Session reset**: when a client goes away, the Service sends
  `Sync` + `ROLLBACK` + `DISCARD ALL` and drains to `ReadyForQuery` before
  reusing the session, so no prepared statements, transactions, or session
  GUCs leak between clients. Sessions that fail to reset are closed.
- **`.cargo/config.toml`** sets `--cfg tokio_unstable`, which tokio requires
  to enable `tokio::net` on `wasm32-wasip2`. sqlx's `runtime-tokio` backend
  uses it to open TCP sockets over `wasi:sockets`.
- **WIT dependencies** are gitignored and fetched by `wash build` into each
  crate's `wit/deps/`. The WASI 0.3 packages come from the registry; the
  template's own `wasmcloud:app` interface lives once at
  [`wasmcloud-app/app.wit`](wasmcloud-app/app.wit) and each crate references
  it as a local path in its `wkg.toml`.

### A note on concurrency

sqlx runs on tokio, and tokio's reactor must be driven, so each backend drives
its query under `block_on` and serializes at the query boundary. The win here
is amortization — bounded, pre-authenticated, shared connections instead of a
fresh TCP + auth round trip per request — not parallel query execution.

### Scope

This is session pooling for a template: the `COPY` sub-protocol,
`CancelRequest`, and TLS on the (in-process) loopback hop are out of scope.
Production traffic to a database outside a trusted network should terminate
TLS at the pool.

# otel-config

A WebAssembly component that demonstrates how to use [OpenTelemetry](https://opentelemetry.io/) within a Wasm component via the `wasi:otel` interfaces. Each operation is instrumented with OTel tracing, logs, and metrics, and the OTel `Resource` is built from `wasi:config` so the same component identifies itself differently per environment.

## Prerequisites

- `cargo` 1.87
- [`wash`](https://wasmcloud.com/docs/installation) 0.x

## Configuration

The runtime values for the dev workload live in [`.wash/config.yaml`](.wash/config.yaml):

- `workload.environment`: env vars for the component (visible via both `wasi:cli/env` and, because wash dev's `wasi:config` plugin runs with `copy_environment = true`, `wasi:config/store::get_all`).
- `workload.config`: opaque key-value injected directly into `wasi:config/store`. Any `otel.resource.*` key here is merged into the OTel `Resource` attributes by `src/otel.rs::build_resource` and rides along with every emitted span, log, and metric.
- `workload.allowedHosts`: outbound HTTP allowlist enforced by wash-runtime.

### Source blocks (`configs:`, `secrets:`)

Top-level entries that `workload.environment.{configFrom,secretFrom}` point
to. Each entry is a *source* with three possible inputs (last-wins on key
conflicts: `inline` → `file` → `fromEnv`):

```yaml
configs:
  app-defaults:
    file: ./config/defaults.env  # KEY=VALUE per line, # comments ok

secrets:
  upstream-credentials:
    fromEnv:
      - UPSTREAM_API_TOKEN       # pulled from your shell at startup
```

## Running

```bash
wash dev
```

In a second terminal:

```bash
curl http://localhost:8000/
```

Expected behavior on the first request:

1. `log_runtime_config_once()` emits a single OTel log line listing every
   `wasi:config/store` key (keys only — with `copy_environment = true`
   secrets land in this map too, and the schema's contract is that values
   are never logged). You'll see `LOG_LEVEL`, `SERVICE_NAME`,
   `OUTBOUND_HOST`, `request_timeout_ms`, `UPSTREAM_API_TOKEN`, plus any
   `otel.resource.*` keys from `workload.config`.
2. The component fetches `https://example.com` (with the `Authorization:
   Bearer <token>` header attached and a 5-second first-byte timeout).
3. The response body is stored in the blobstore.
4. The keyvalue counter increments and is returned as the response body.

`secrets.upstream-credentials.fromEnv` requires `UPSTREAM_API_TOKEN` to be
set in the shell that runs `wash dev`. Without it, `wash dev` fails fast with:

```bash
failed to resolve workload-level configuration

Caused by:
    0: failed to resolve workload.environment.secretFrom
    1: source 'upstream-credentials' references environment variable 'UPSTREAM_API_TOKEN' which is not set
```

### Option A: direnv (recommended)

[direnv](https://direnv.net) auto-loads exports from `.envrc` whenever you
`cd` into the project. The example ships an `.envrc.example` template; copy
it once and authorize:

```bash
cp .envrc.example .envrc
direnv allow
```

After this, `cd`-ing into the project directory loads `UPSTREAM_API_TOKEN`
automatically. `.envrc` is gitignored, so real values you put there stay
out of git.

```bash
wash dev
```

### Option B: plain shell export

```bash
export UPSTREAM_API_TOKEN=demo-token-12345
wash dev
```

## Required Capabilities

1. `wasi:http` to receive incoming requests and call `https://example.com`
2. `wasi:blobstore` to stash the response body
3. `wasi:keyvalue` to increment a request counter
4. `wasi:config` to populate the OTel `Resource` attributes
5. `wasi:random` for W3C trace and span IDs
6. `wasi:clocks/wall-clock` for telemetry timestamps
7. `wasi:otel/{tracing,logs,metrics}` to export telemetry

## OpenTelemetry behavior

### Tracing

- Retrieves the host's span context with `outer-span-context` and inherits its sampling decision
- Creates a parent `handle-request` server span covering the full request lifecycle
- Creates child client spans for each downstream operation (HTTP fetch, blobstore write, keyvalue increment)
- Attaches semantic attributes and span events to each span
- Injects W3C `traceparent` on the outgoing HTTP request so the trace continues into the downstream service

### Logs

- Emits trace-correlated `LogRecord`s via `wasi:otel/logs` `on-emit` so log lines render under the right span in the OTel backend

### Metrics

- Exports a monotonic `U64Sum` counter tracking the total number of requests handled
- Exports a `U64Gauge` capturing the latest HTTP response body size

## End-to-end with an OTLP-compatible viewer

The fastest way to see traces, logs, and metrics from this component end to end is to point it at the
[Aspire dashboard](https://learn.microsoft.com/en-us/dotnet/aspire/fundamentals/dashboard/standalone),
which is a standalone OTLP-compatible viewer that ships as a single Docker image.

In one terminal, run the dashboard:

```shell
docker run --rm -it \
  -p 18888:18888 \
  -p 18889:18889 \
  -e DOTNET_DASHBOARD_UNSECURED_ALLOW_ANONYMOUS=true \
  --name aspire-dashboard \
  mcr.microsoft.com/dotnet/aspire-dashboard:latest
```

- `18888` — dashboard UI (open in browser)
- `18889` — OTLP gRPC ingest endpoint

In a second terminal, point `wash dev` at the dashboard's OTLP endpoint. wash-runtime's
exporter activates whenever any `OTEL_*` env var is set and uses gRPC by default:

```shell
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:18889 wash dev
```

`OTEL_EXPORTER_OTLP_ENDPOINT` configures wash itself (it's read from wash dev's process env),
so it stays on the command line. The Resource identity is sourced from
[`.wash/config.yaml`](.wash/config.yaml) — see the [Configuration](#configuration) section above.

In a third terminal, trigger a request:

```shell
curl http://localhost:8000/
```

Open the dashboard at [http://localhost:18888](http://localhost:18888). You should see:

- **Traces**: a trace rooted in the `handle-request` server span with child client spans for
  `outgoing http`, `blobstore write`, and `keyvalue increment`. The outgoing HTTP call carries
  W3C `traceparent` so the trace continues into any downstream service that participates.
- **Structured logs**: `INFO` records emitted at startup (the `wasi:config` key list), at the
  start of the request, after the fetch, and on completion — each correlated to the
  `handle-request` span.
- **Metrics**: the cumulative `http.server.request_count` counter and the latest
  `http.server.response_body.size` gauge, both labelled by the Resource attributes from
  `workload.config`.

### Demo: `allowedHosts` in action

Edit `config/defaults.env` and change:

```
OUTBOUND_HOST=httpbin.org
```

Restart `wash dev` and hit the endpoint again. The component will return
HTTP 500 because the outbound request is blocked at the host —
`httpbin.org` isn't in `workload.allowedHosts`. Add it to the allowlist:

```yaml
workload:
  allowedHosts:
    - example.com
    - httpbin.org
```

Restart, retry, and the request succeeds.

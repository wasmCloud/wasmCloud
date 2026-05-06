//! `OTel` config example.
//!
//! Handles an incoming HTTP request, fetches `https://example.com`, stashes
//! the body in a blob, increments a request counter, and exports the whole
//! flow through `wasi:otel/{tracing,logs,metrics}`, including W3C trace
//! context propagation on the outbound call. The OTel `Resource` is built
//! from `wasi:config` (`otel.resource.*` keys).
//!
//! The reusable OTel scaffolding (cached resource/scope, [`otel_log`],
//! [`ActiveSpan`] RAII guard) lives in [`mod@otel`].

use std::collections::BTreeMap;
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};
use wstd::http::{Body, Client, HeaderValue, Request, Response, StatusCode};
use wstd::time::Duration;

mod bindings {
    wit_bindgen::generate!({
        world: "otel-config",
        path: "wit",
        generate_all,
    });
}

mod otel;

use bindings::wasi::blobstore::{
    blobstore::{create_container, get_container},
    types::OutgoingValue,
};
use bindings::wasi::config::store::get_all;
use bindings::wasi::keyvalue::{atomics::increment, store::open};
use bindings::wasi::otel as otel_bindings;
use otel::{
    ActiveSpan, Event, SpanKind, TraceFlags, component_start, kv_num, kv_str, new_trace_id, now,
    otel_log, outer_span_context, resource, scope, traceparent,
};

const CONTAINER_NAME: &str = "http-responses";
const OBJECT_KEY: &str = "example-com-response";
const COUNTER_KEY: &str = "request-count";

// Defaults applied if the corresponding `wasi:config` key is absent.
const DEFAULT_OUTBOUND_HOST: &str = "example.com";
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 5_000;

/// Per-request app config sourced from `wasi:config/store` (populated by
/// `workload.environment.{configFrom,secretFrom}` in `.wash/config.yaml`).
/// Built once on the first request and cached.
struct AppConfig {
    outbound_url: String,
    request_timeout: Duration,
    upstream_api_token: Option<String>,
}

static APP_CONFIG: OnceLock<AppConfig> = OnceLock::new();

fn app_config() -> &'static AppConfig {
    APP_CONFIG.get_or_init(build_app_config)
}

fn build_app_config() -> AppConfig {
    let mut entries: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(config) = get_all() {
        for (k, v) in config {
            entries.insert(k, v);
        }
    }
    let outbound_host = entries
        .get("OUTBOUND_HOST")
        .map(String::as_str)
        .unwrap_or(DEFAULT_OUTBOUND_HOST);
    let timeout_ms = entries
        .get("request_timeout_ms")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS);
    AppConfig {
        outbound_url: format!("https://{outbound_host}/"),
        request_timeout: Duration::from_millis(timeout_ms),
        upstream_api_token: entries.get("UPSTREAM_API_TOKEN").cloned(),
    }
}

/// One-shot startup audit: emit a single OTel log line listing every
/// `wasi:config/store` key visible to the component. **Keys only**,
/// values are deliberately not included because secret values
/// (`workload.environment.secretFrom` entries) land in this same map.
fn log_runtime_config(trace_id: &str, span_id: &str, flags: TraceFlags) {
    static LOGGED: OnceLock<()> = OnceLock::new();
    LOGGED.get_or_init(|| {
        let keys: Vec<String> = match get_all() {
            Ok(entries) => entries.into_iter().map(|(k, _)| k).collect(),
            Err(e) => {
                eprintln!("warning: failed to read wasi:config/store: {e:?}");
                return;
            }
        };
        otel_log(
            &format!("wasi:config keys: {}", keys.join(", ")),
            trace_id,
            span_id,
            flags,
        );
    });
}

#[wstd::http_server]
async fn main(_request: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    match handle_request().await {
        Ok(count) => Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(count))
            .map_err(Into::into),
        Err(e) => {
            eprintln!("error processing request: {e:?}");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(format!("Internal server error: {e}")))
                .map_err(Into::into)
        }
    }
}

async fn handle_request() -> Result<String> {
    // Continue the host-provided trace if there is one; otherwise root a fresh
    // trace. Inherit the upstream sampling decision so we don't override a
    // host that asked us not to record.
    let outer_ctx = outer_span_context();
    let (trace_id, flags, outer_parent_span_id) = if outer_ctx.trace_id.is_empty() {
        (new_trace_id(), TraceFlags::SAMPLED, String::new())
    } else {
        (outer_ctx.trace_id, outer_ctx.trace_flags, outer_ctx.span_id)
    };

    let request_span = ActiveSpan::start(
        &trace_id,
        flags,
        outer_parent_span_id,
        SpanKind::Server,
        "handle-request",
    );
    let request_span_id = request_span.span_id().to_owned();

    log_runtime_config(&trace_id, &request_span_id, flags);

    otel_log(
        "handling otel-config request",
        &trace_id,
        &request_span_id,
        flags,
    );

    // Outgoing http child CLIENT span, traceparent injected on the wire.
    let outbound_url = &app_config().outbound_url;
    let http_span = ActiveSpan::start(
        &trace_id,
        flags,
        &request_span_id,
        SpanKind::Client,
        "outgoing http",
    );
    let http_span_id = http_span.span_id().to_owned();
    let body_bytes = make_outgoing_request(&trace_id, &http_span_id, flags).await?;
    let response_len = body_bytes.len();
    http_span.finish_ok(vec![
        kv_str("http.request.method", "GET"),
        kv_str("url.full", outbound_url),
        kv_num("http.response.body.size", response_len),
    ]);

    otel_log(
        &format!("retrieved {response_len} bytes from {outbound_url}"),
        &trace_id,
        &request_span_id,
        flags,
    );

    // Blobstore write — child CLIENT span.
    let blob_span = ActiveSpan::start(
        &trace_id,
        flags,
        &request_span_id,
        SpanKind::Client,
        "blobstore write",
    );
    store_response_in_blobstore(&body_bytes)?;
    blob_span.finish_ok(vec![
        kv_str("blobstore.container", CONTAINER_NAME),
        kv_str("blobstore.object", OBJECT_KEY),
        kv_num("blobstore.object.size", response_len),
    ]);

    // Keyvalue increment child CLIENT span.
    let kv_span = ActiveSpan::start(
        &trace_id,
        flags,
        &request_span_id,
        SpanKind::Client,
        "keyvalue increment",
    );
    let count = increment_counter()?;
    kv_span.finish_ok(vec![
        kv_str("keyvalue.key", COUNTER_KEY),
        kv_num("keyvalue.result", count),
    ]);

    // Parent SERVER span carries summary attributes + a completion event.
    request_span.finish_ok_with(
        vec![kv_num("request.count", count)],
        vec![Event {
            name: "request.completed".into(),
            time: now(),
            attributes: vec![kv_num("response.bytes", response_len)],
        }],
    );

    otel_log(
        &format!("request complete, count: {count}"),
        &trace_id,
        &request_span_id,
        flags,
    );

    export_metrics(count, response_len);
    Ok(count.to_string())
}

fn export_metrics(count: u64, response_len: usize) {
    use otel_bindings::metrics::{
        Gauge, GaugeDataPoint, Metric, MetricData, MetricNumber, ResourceMetrics, ScopeMetrics,
        Sum, SumDataPoint, Temporality, export,
    };

    let metric_time = now();
    let start_time = component_start();
    let _ = export(&ResourceMetrics {
        resource: resource().clone(),
        scope_metrics: vec![ScopeMetrics {
            scope: scope().clone(),
            metrics: vec![
                Metric {
                    name: "http.server.request_count".into(),
                    description: "Total number of HTTP requests handled".into(),
                    unit: "{request}".into(),
                    data: MetricData::U64Sum(Sum {
                        data_points: vec![SumDataPoint {
                            attributes: vec![],
                            value: MetricNumber::U64(count),
                            exemplars: vec![],
                        }],
                        // Per OTel spec, the first cumulative observation may
                        // legitimately carry start_time == time; subsequent
                        // exports keep the same start_time.
                        start_time,
                        time: metric_time,
                        temporality: Temporality::Cumulative,
                        is_monotonic: true,
                    }),
                },
                Metric {
                    name: "http.server.response_body.size".into(),
                    description: "Size of the fetched HTTP response body".into(),
                    unit: "By".into(),
                    data: MetricData::U64Gauge(Gauge {
                        data_points: vec![GaugeDataPoint {
                            attributes: vec![kv_str("http.target", &app_config().outbound_url)],
                            value: MetricNumber::U64(response_len as u64),
                            exemplars: vec![],
                        }],
                        start_time: Some(start_time),
                        time: metric_time,
                    }),
                },
            ],
        }],
    });
}

async fn make_outgoing_request(
    trace_id: &str,
    span_id: &str,
    flags: TraceFlags,
) -> Result<Vec<u8>> {
    let cfg = app_config();
    let mut client = Client::new();
    client.set_first_byte_timeout(cfg.request_timeout);
    client.set_between_bytes_timeout(cfg.request_timeout);

    let header = traceparent(trace_id, span_id, flags);
    let mut builder = Request::get(&cfg.outbound_url).header(
        "traceparent",
        HeaderValue::from_str(&header).context("invalid traceparent header")?,
    );

    // Attach the upstream API token from `secrets.upstream-credentials`.
    // Missing-token paths still work but skip the auth header so the example
    // is exercisable without setting `UPSTREAM_API_TOKEN`.
    if let Some(token) = cfg.upstream_api_token.as_deref() {
        builder = builder.header(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}"))
                .context("invalid authorization header")?,
        );
    }

    let request = builder
        .body(Body::empty())
        .context("failed to build outgoing request")?;

    let response = client
        .send(request)
        .await
        .context("outgoing request failed")?;

    let status = response.status();
    if !status.is_success() {
        bail!("non-success status from {}: {status}", cfg.outbound_url);
    }

    let mut body = response.into_body();
    let bytes = body
        .contents()
        .await
        .context("failed to read response body")?
        .to_vec();
    Ok(bytes)
}

fn store_response_in_blobstore(body: &[u8]) -> Result<()> {
    let container = get_container(CONTAINER_NAME).or_else(|_| {
        create_container(CONTAINER_NAME)
            .map_err(|e| anyhow!("failed to create blobstore container {CONTAINER_NAME}: {e}"))
    })?;

    let outgoing = OutgoingValue::new_outgoing_value();
    let stream = outgoing
        .outgoing_value_write_body()
        .map_err(|()| anyhow!("failed to open blobstore output stream"))?;
    stream
        .blocking_write_and_flush(body)
        .context("failed to write blob bytes")?;
    drop(stream);

    container
        .write_data(OBJECT_KEY, &outgoing)
        .map_err(|e| anyhow!("failed to write blob {OBJECT_KEY}: {e}"))?;
    OutgoingValue::finish(outgoing).map_err(|e| anyhow!("failed to finish blob value: {e}"))?;
    Ok(())
}

fn increment_counter() -> Result<u64> {
    let bucket = open("").map_err(|e| anyhow!("failed to open keyvalue bucket: {e:?}"))?;
    increment(&bucket, COUNTER_KEY, 1).context("failed to increment counter")
}

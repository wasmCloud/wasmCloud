//! `OTel` config example.
//!
//! Handles an incoming HTTP request, fetches the configured upstream, stashes
//! the body in a blob, increments a request counter, and exports the whole
//! flow through `wasi:otel/{tracing,logs,metrics}`, including W3C trace
//! context propagation on the outbound call. The OTel `Resource` is built
//! from `wasi:config` (`otel.resource.*` keys).
//!
//! The reusable OTel scaffolding (cached resource/scope, [`otel::otel_log`],
//! [`otel::ActiveSpan`] RAII guard, and all `wasi:otel` bindings re-exports)
//! lives in [`mod@otel`]. Per-capability helpers live in their own modules
//! (`config`, `outbound`, `blobstore`, `keyvalue`, `metrics`, `ui`).

use anyhow::{Error, Result};
use wstd::http::{Body, Request, Response, StatusCode};

mod bindings {
    wit_bindgen::generate!({
        world: "otel-config",
        path: "wit",
        generate_all,
    });
}

mod blobstore;
mod config;
mod keyvalue;
mod metrics;
mod otel;
mod outbound;
mod ui;

use blobstore::{CONTAINER_NAME, OBJECT_KEY, store_response_in_blobstore};
use config::{app_config, log_runtime_config, otlp_endpoint};
use keyvalue::{COUNTER_KEY, increment_counter};
use metrics::export_metrics;
use otel::{
    ActiveSpan, Event, SpanKind, TraceFlags, kv_num, kv_str, new_trace_id, now, otel_log,
    outer_span_context,
};
use outbound::make_outgoing_request;

/// Everything the response renderer needs to draw the demo page.
struct Outcome {
    count: u64,
    response_len: usize,
    outbound_url: String,
    trace_id: String,
    request_span_id: String,
}

#[wstd::http_server]
async fn main(_request: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let (status, body) = match handle_request().await {
        Ok(outcome) => {
            let page = ui::render_page(&ui::PageData {
                count: outcome.count,
                response_len: outcome.response_len,
                outbound_url: &outcome.outbound_url,
                trace_id: &outcome.trace_id,
                span_id: &outcome.request_span_id,
                otlp_endpoint: otlp_endpoint(),
            });
            (StatusCode::OK, page)
        }
        Err(e) => {
            eprintln!("error processing request: {e:?}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ui::render_error(&format!("{e:#}")),
            )
        }
    };

    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(body))
        .map_err(Into::into)
}

async fn handle_request() -> Result<Outcome> {
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

    match handle_request_steps(&trace_id, &request_span_id, flags).await {
        Ok((count, response_len, outbound_url)) => {
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

            export_metrics(count, response_len, &outbound_url);
            Ok(Outcome {
                count,
                response_len,
                outbound_url,
                trace_id,
                request_span_id,
            })
        }
        Err(e) => {
            request_span.finish_err(format!("{e:#}"));
            Err(e)
        }
    }
}

/// Inner workhorse: runs the http → blob → keyvalue chain, marking each
/// child span as either `Ok` or `Err(message)` so the trace carries the real
/// cause instead of the generic "exited via early return" message.
async fn handle_request_steps(
    trace_id: &str,
    parent_span_id: &str,
    flags: TraceFlags,
) -> Result<(u64, usize, String)> {
    let outbound_url = app_config().outbound_url.clone();

    // Outgoing http child CLIENT span, traceparent injected on the wire.
    let http_span = ActiveSpan::start(
        trace_id,
        flags,
        parent_span_id,
        SpanKind::Client,
        "outgoing http",
    );
    let http_span_id = http_span.span_id().to_owned();
    let body_bytes = finish_span_with_result(
        http_span,
        make_outgoing_request(trace_id, &http_span_id, flags).await,
        |b| {
            vec![
                kv_str("http.request.method", "GET"),
                kv_str("url.full", &outbound_url),
                kv_num("http.response.body.size", b.len()),
            ]
        },
    )?;
    let response_len = body_bytes.len();

    otel_log(
        &format!("retrieved {response_len} bytes from {outbound_url}"),
        trace_id,
        parent_span_id,
        flags,
    );

    // Blobstore write — child CLIENT span.
    let blob_span = ActiveSpan::start(
        trace_id,
        flags,
        parent_span_id,
        SpanKind::Client,
        "blobstore write",
    );
    finish_span_with_result(blob_span, store_response_in_blobstore(&body_bytes), |()| {
        vec![
            kv_str("blobstore.container", CONTAINER_NAME),
            kv_str("blobstore.object", OBJECT_KEY),
            kv_num("blobstore.object.size", response_len),
        ]
    })?;

    // Keyvalue increment child CLIENT span.
    let kv_span = ActiveSpan::start(
        trace_id,
        flags,
        parent_span_id,
        SpanKind::Client,
        "keyvalue increment",
    );
    let count = finish_span_with_result(kv_span, increment_counter(), |count| {
        vec![
            kv_str("keyvalue.key", COUNTER_KEY),
            kv_num("keyvalue.result", *count),
        ]
    })?;

    Ok((count, response_len, outbound_url))
}

/// Settle `span` based on `result`: on `Ok`, attach `ok_attrs(&value)` and
/// mark `Status::Ok`; on `Err`, record the error message on the span before
/// returning the error to the caller.
fn finish_span_with_result<T>(
    span: ActiveSpan,
    result: Result<T>,
    ok_attrs: impl FnOnce(&T) -> Vec<otel::KeyValue>,
) -> Result<T> {
    match result {
        Ok(v) => {
            span.finish_ok(ok_attrs(&v));
            Ok(v)
        }
        Err(e) => {
            span.finish_err(format!("{e:#}"));
            Err::<T, Error>(e)
        }
    }
}

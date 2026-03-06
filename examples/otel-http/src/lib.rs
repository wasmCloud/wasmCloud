use std::cell::Cell;

use anyhow::{Context, Result};

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::{
    exports::wasi::http::incoming_handler::Guest,
    wasi::{
        blobstore::{
            blobstore::{create_container, get_container},
            types::OutgoingValue,
        },
        config::store::get_all,
        http::{
            outgoing_handler::{handle, OutgoingRequest, RequestOptions},
            types::{
                Fields, IncomingRequest, Method, OutgoingBody, OutgoingResponse, ResponseOutparam,
                Scheme,
            },
        },
        keyvalue::{atomics::increment, store::open},
        logging::logging::{log, Level},
    },
};

use bindings::wasi::clocks0_2_0::wall_clock;
use bindings::wasi::otel;

struct Component;

const CONTAINER_NAME: &str = "http-responses";
const OBJECT_KEY: &str = "example-com-response";
const COUNTER_KEY: &str = "request-count";

thread_local! {
    static SPAN_COUNTER: Cell<u64> = const { Cell::new(1) };
}

/// Generate a unique span ID (8 bytes as 16-char hex string).
fn new_span_id() -> String {
    SPAN_COUNTER.with(|c| {
        let id = c.get();
        c.set(id + 1);
        format!("{id:016x}")
    })
}

/// Get the current wall-clock time (wasi:clocks@0.2.0 for otel compatibility).
fn now() -> wall_clock::Datetime {
    wall_clock::now()
}

/// Create the instrumentation scope for this component.
fn scope() -> otel::types::InstrumentationScope {
    otel::types::InstrumentationScope {
        name: "otel-http".into(),
        version: Some("0.1.0".into()),
        schema_url: None,
        attributes: vec![],
    }
}

/// Create the resource descriptor for this component.
fn resource() -> otel::types::Resource {
    otel::types::Resource {
        attributes: vec![otel::types::KeyValue {
            key: "service.name".into(),
            value: "\"otel-http\"".into(),
        }],
        schema_url: None,
    }
}

/// Emit a structured OTel log record correlated with the current trace.
fn otel_log(severity: u8, severity_text: &str, body: &str, trace_id: &str, span_id: &str) {
    otel::logs::on_emit(&otel::logs::LogRecord {
        timestamp: Some(now()),
        observed_timestamp: None,
        severity_text: Some(severity_text.into()),
        severity_number: Some(severity),
        body: Some(body.into()),
        attributes: None,
        event_name: None,
        resource: Some(resource()),
        instrumentation_scope: Some(scope()),
        trace_id: Some(trace_id.into()),
        span_id: Some(span_id.into()),
        trace_flags: None,
    });
}

/// Start a new child span and return its context + start time.
fn start_span(trace_id: &str) -> (otel::tracing::SpanContext, wall_clock::Datetime) {
    let ctx = otel::tracing::SpanContext {
        trace_id: trace_id.into(),
        span_id: new_span_id(),
        trace_flags: otel::tracing::TraceFlags::SAMPLED,
        is_remote: false,
        trace_state: vec![],
    };
    otel::tracing::on_start(&ctx);
    let start = now();
    (ctx, start)
}

/// End a span with the given metadata.
fn end_span(
    ctx: otel::tracing::SpanContext,
    parent_span_id: &str,
    kind: otel::tracing::SpanKind,
    name: &str,
    start_time: wall_clock::Datetime,
    attributes: Vec<otel::types::KeyValue>,
    events: Vec<otel::tracing::Event>,
    status: otel::tracing::Status,
) {
    otel::tracing::on_end(&otel::tracing::SpanData {
        span_context: ctx,
        parent_span_id: parent_span_id.into(),
        span_kind: kind,
        name: name.into(),
        start_time,
        end_time: now(),
        attributes,
        events,
        links: vec![],
        status,
        instrumentation_scope: scope(),
        dropped_attributes: 0,
        dropped_events: 0,
        dropped_links: 0,
    });
}

/// Helper to create an OTel key-value attribute with a JSON string value.
fn kv(key: &str, value: &str) -> otel::types::KeyValue {
    otel::types::KeyValue {
        key: key.into(),
        value: format!("\"{value}\""),
    }
}

/// Helper to create an OTel key-value attribute with a numeric value.
fn kv_num(key: &str, value: impl std::fmt::Display) -> otel::types::KeyValue {
    otel::types::KeyValue {
        key: key.into(),
        value: value.to_string(),
    }
}

impl Guest for Component {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        match handle_request() {
            Ok(count) => {
                log(Level::Info, "", &format!("Successfully processed request, count: {count}"));
                let response = OutgoingResponse::new(Fields::new());
                response.set_status_code(200).unwrap();
                let body = response.body().unwrap();
                ResponseOutparam::set(response_out, Ok(response));

                let stream = body.write().unwrap();
                stream.blocking_write_and_flush(count.as_bytes()).unwrap();
                drop(stream);
                OutgoingBody::finish(body, None).unwrap();
            }
            Err(e) => {
                log(Level::Error, "", &format!("Error processing request: {e}"));
                let response = OutgoingResponse::new(Fields::new());
                response.set_status_code(500).unwrap();
                let body = response.body().unwrap();
                ResponseOutparam::set(response_out, Ok(response));

                let error_msg = format!("Internal server error: {e}");
                let stream = body.write().unwrap();
                stream.blocking_write_and_flush(error_msg.as_bytes()).unwrap();
                drop(stream);
                OutgoingBody::finish(body, None).unwrap();
            }
        }
    }
}

fn handle_request() -> Result<String> {
    log(Level::Info, "", "Starting HTTP counter request processing");

    // --- OTel Tracing: get host span context and start a parent span ---
    let outer_ctx = otel::tracing::outer_span_context();
    let trace_id = outer_ctx.trace_id.clone();
    let (request_ctx, request_start) = start_span(&trace_id);
    let request_span_id = request_ctx.span_id.clone();

    // --- OTel Logging: emit a structured log correlated with the trace ---
    otel_log(
        9, // INFO severity
        "INFO",
        "\"Starting HTTP counter request processing\"",
        &trace_id,
        &request_span_id,
    );

    log_runtime_config()?;

    // --- OTel Tracing: child span for outgoing HTTP request ---
    let (http_ctx, http_start) = start_span(&trace_id);
    let response_body = make_outgoing_request()?;
    let response_len = response_body.len();
    end_span(
        http_ctx,
        &request_span_id,
        otel::tracing::SpanKind::Client,
        "GET example.com",
        http_start,
        vec![
            kv("http.method", "GET"),
            kv("http.url", "https://example.com/"),
            kv_num("http.response.body.size", response_len),
        ],
        vec![],
        otel::tracing::Status::Ok,
    );

    otel_log(
        9,
        "INFO",
        &format!("\"Retrieved {response_len} bytes from example.com\""),
        &trace_id,
        &request_span_id,
    );

    // --- OTel Tracing: child span for blobstore write ---
    let (blob_ctx, blob_start) = start_span(&trace_id);
    store_response_in_blobstore(&response_body)?;
    end_span(
        blob_ctx,
        &request_span_id,
        otel::tracing::SpanKind::Client,
        "blobstore write",
        blob_start,
        vec![
            kv("blobstore.container", CONTAINER_NAME),
            kv("blobstore.object", OBJECT_KEY),
        ],
        vec![],
        otel::tracing::Status::Ok,
    );

    // --- OTel Tracing: child span for keyvalue increment ---
    let (kv_ctx, kv_start) = start_span(&trace_id);
    let count = increment_counter()?;
    end_span(
        kv_ctx,
        &request_span_id,
        otel::tracing::SpanKind::Client,
        "keyvalue increment",
        kv_start,
        vec![
            kv("keyvalue.key", COUNTER_KEY),
            kv_num("keyvalue.result", count),
        ],
        vec![],
        otel::tracing::Status::Ok,
    );

    // --- OTel Tracing: end the parent request span with an event ---
    end_span(
        request_ctx,
        &outer_ctx.span_id,
        otel::tracing::SpanKind::Server,
        "handle-request",
        request_start,
        vec![kv_num("request.count", count)],
        vec![otel::tracing::Event {
            name: "request.completed".into(),
            time: now(),
            attributes: vec![kv_num("response.bytes", response_len)],
        }],
        otel::tracing::Status::Ok,
    );

    // --- OTel Metrics: export request counter and response size gauge ---
    let metric_time = now();
    let _ = otel::metrics::export(&otel::metrics::ResourceMetrics {
        resource: resource(),
        scope_metrics: vec![otel::metrics::ScopeMetrics {
            scope: scope(),
            metrics: vec![
                // Monotonic counter tracking total requests handled
                otel::metrics::Metric {
                    name: "http.server.request_count".into(),
                    description: "Total number of HTTP requests handled".into(),
                    unit: "{request}".into(),
                    data: otel::metrics::MetricData::U64Sum(otel::metrics::Sum {
                        data_points: vec![otel::metrics::SumDataPoint {
                            attributes: vec![],
                            value: otel::metrics::MetricNumber::U64(count),
                            exemplars: vec![],
                        }],
                        start_time: metric_time,
                        time: metric_time,
                        temporality: otel::metrics::Temporality::Cumulative,
                        is_monotonic: true,
                    }),
                },
                // Gauge capturing the latest response body size
                otel::metrics::Metric {
                    name: "http.server.response_body.size".into(),
                    description: "Size of the fetched HTTP response body".into(),
                    unit: "By".into(),
                    data: otel::metrics::MetricData::U64Gauge(otel::metrics::Gauge {
                        data_points: vec![otel::metrics::GaugeDataPoint {
                            attributes: vec![kv("http.target", "https://example.com/")],
                            value: otel::metrics::MetricNumber::U64(response_len as u64),
                            exemplars: vec![],
                        }],
                        start_time: Some(metric_time),
                        time: metric_time,
                    }),
                },
            ],
        }],
    });

    otel_log(
        9,
        "INFO",
        &format!("\"Request complete, count: {count}\""),
        &trace_id,
        &request_span_id,
    );

    Ok(count.to_string())
}

fn log_runtime_config() -> Result<()> {
    log(Level::Info, "", "Retrieving runtime configuration");
    match get_all() {
        Ok(config) => {
            log(Level::Info, "", "Runtime configuration keys available:");
            for (key, value) in config.iter() {
                log(Level::Info, "", &format!("Config key: {key} = {value}"));
            }
            if config.is_empty() {
                log(Level::Info, "", "No runtime configuration keys found");
            }
        }
        Err(e) => {
            log(Level::Warn, "", &format!("Failed to retrieve runtime configuration: {e:?}"));
        }
    }
    Ok(())
}

fn make_outgoing_request() -> Result<String> {
    log(Level::Info, "", "Making outgoing HTTP request to https://example.com");

    let request = OutgoingRequest::new(Fields::new());
    request
        .set_scheme(Some(&Scheme::Https))
        .map_err(|_| anyhow::anyhow!("Failed to set HTTPS scheme"))?;
    request
        .set_authority(Some("example.com"))
        .map_err(|_| anyhow::anyhow!("Failed to set authority"))?;
    request
        .set_path_with_query(Some("/"))
        .map_err(|_| anyhow::anyhow!("Failed to set path"))?;
    request
        .set_method(&Method::Get)
        .map_err(|_| anyhow::anyhow!("Failed to set GET method"))?;

    let options = RequestOptions::new();

    let future_response =
        handle(request, Some(options)).context("Failed to initiate outgoing request")?;

    future_response.subscribe().block();

    let incoming_response = future_response
        .get()
        .context("Failed to get response from future")?
        .map_err(|e| anyhow::anyhow!("Request failed: {e:?}"))??;

    let status = incoming_response.status();
    log(Level::Info, "", &format!("Received response with status: {status}"));

    if status < 200 || status >= 300 {
        log(Level::Warn, "", &format!("Non-success status code received: {status}"));
    }

    let body_stream = incoming_response
        .consume()
        .map_err(|_| anyhow::anyhow!("Failed to consume response body"))?;

    let input_stream = body_stream
        .stream()
        .map_err(|_| anyhow::anyhow!("Failed to get input stream"))?;

    let mut body_data = Vec::new();
    loop {
        match input_stream.read(8192) {
            Ok(chunk) => {
                if chunk.is_empty() {
                    break;
                }
                body_data.extend_from_slice(&chunk);
            }
            Err(_) => break,
        }
    }

    let body_string = String::from_utf8_lossy(&body_data).to_string();
    log(Level::Info, "", &format!("Retrieved {} bytes from example.com", body_data.len()));

    Ok(body_string)
}

fn store_response_in_blobstore(response_body: &str) -> Result<()> {
    log(Level::Info, "", &format!("Storing response in blobstore container: {CONTAINER_NAME}"));

    let container = match get_container(CONTAINER_NAME) {
        Ok(container) => {
            log(Level::Info, "", &format!("Using existing container: {CONTAINER_NAME}"));
            container
        }
        Err(_) => {
            log(Level::Info, "", &format!("Creating new container: {CONTAINER_NAME}"));
            create_container(CONTAINER_NAME)
                .map_err(|e| anyhow::anyhow!("Failed to create blobstore container: {e}"))?
        }
    };

    let response_bytes = response_body.as_bytes();

    let outgoing_value = OutgoingValue::new_outgoing_value();
    let output_stream = outgoing_value
        .outgoing_value_write_body()
        .map_err(|_| anyhow::anyhow!("Failed to get output stream"))?;

    output_stream
        .blocking_write_and_flush(response_bytes)
        .context("Failed to write data")?;
    drop(output_stream);

    container
        .write_data(OBJECT_KEY, &outgoing_value)
        .map_err(|e| anyhow::anyhow!("Failed to store response in blobstore: {e}"))?;

    OutgoingValue::finish(outgoing_value).ok();

    log(Level::Info, "", &format!("Successfully stored {} bytes in object: {OBJECT_KEY}", response_bytes.len()));

    Ok(())
}

fn increment_counter() -> Result<u64> {
    log(Level::Info, "", "Incrementing request counter");

    let bucket = open("")?;

    let new_count = increment(&bucket, COUNTER_KEY, 1).context("Failed to increment counter")?;

    log(Level::Info, "", &format!("Request count incremented to: {new_count}"));

    Ok(new_count)
}

bindings::export!(Component with_types_in bindings);

//! Reusable `OTel` scaffolding for this example.
//!
//! Cached `Resource` and `InstrumentationScope` built once from `wasi:config`,
//! a single trace-correlated log helper ([`otel_log`]), and an RAII
//! [`ActiveSpan`] guard that ends spans even when a `?` early-returns past
//! the explicit completion call.
//!
//! Pulled out of `lib.rs` so the demo flow there reads as a story rather
//! than a sea of OTel plumbing.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

use crate::bindings::wasi::{
    clocks::wall_clock, config::store::get_all, otel, random::random::get_random_bytes,
};

pub(crate) use crate::bindings::wasi::otel::tracing::{
    Event, SpanKind, TraceFlags, outer_span_context,
};

const PKG_NAME: &str = env!("CARGO_PKG_NAME");
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Config-key prefix that gets merged into the OTel `Resource` attributes.
/// Mirrors the upstream `OTEL_RESOURCE_ATTRIBUTES` convention.
const RESOURCE_CONFIG_PREFIX: &str = "otel.resource.";

// --- Time ---

/// Captured at first observation; used as the cumulative-counter start time.
static COMPONENT_START: OnceLock<wall_clock::Datetime> = OnceLock::new();

pub(crate) fn now() -> wall_clock::Datetime {
    wall_clock::now()
}

pub(crate) fn component_start() -> wall_clock::Datetime {
    *COMPONENT_START.get_or_init(now)
}

// --- W3C trace context IDs ---

fn random_hex(bytes: usize) -> String {
    let buf = get_random_bytes(bytes as u64);
    let mut out = String::with_capacity(bytes * 2);
    for b in &buf {
        let _ = write!(out, "{b:02x}");
    }
    out
}

pub(crate) fn new_trace_id() -> String {
    random_hex(16)
}
fn new_span_id() -> String {
    random_hex(8)
}

/// Format a W3C `traceparent` header value: `00-{trace_id}-{span_id}-{flags}`.
pub(crate) fn traceparent(trace_id: &str, span_id: &str, flags: TraceFlags) -> String {
    let flag_byte = u8::from(flags.contains(TraceFlags::SAMPLED));
    format!("00-{trace_id}-{span_id}-{flag_byte:02x}")
}

// --- Encoding helpers ---

/// `wasi:otel/types.value` is JSON-encoded — strings need to be quoted/escaped.
fn json_str(s: &str) -> String {
    serde_json::to_string(s).expect("string is always JSON-encodable")
}

pub(crate) fn kv_str(key: &str, value: &str) -> otel::types::KeyValue {
    otel::types::KeyValue {
        key: key.into(),
        value: json_str(value),
    }
}

pub(crate) fn kv_num(key: &str, value: impl std::fmt::Display) -> otel::types::KeyValue {
    otel::types::KeyValue {
        key: key.into(),
        value: value.to_string(),
    }
}

// --- Resource + InstrumentationScope (cached) ---

static RESOURCE: OnceLock<otel::types::Resource> = OnceLock::new();
static SCOPE: OnceLock<otel::types::InstrumentationScope> = OnceLock::new();

/// Cached OTel `Resource`. Built once on first call by reading `wasi:config`:
/// any key prefixed with `otel.resource.` is merged in (mirrors the upstream
/// `OTEL_RESOURCE_ATTRIBUTES` convention). `service.name` and
/// `service.version` default to the crate name/version unless overridden.
pub(crate) fn resource() -> &'static otel::types::Resource {
    RESOURCE.get_or_init(build_resource)
}

fn build_resource() -> otel::types::Resource {
    // BTreeMap so the wire-format attribute order is deterministic across runs.
    let mut attrs: BTreeMap<String, String> = BTreeMap::new();
    attrs.insert("service.name".into(), PKG_NAME.into());
    attrs.insert("service.version".into(), PKG_VERSION.into());

    match get_all() {
        Ok(config) => {
            for (key, value) in config {
                if let Some(attr_key) = key.strip_prefix(RESOURCE_CONFIG_PREFIX)
                    && !attr_key.is_empty()
                {
                    attrs.insert(attr_key.into(), value);
                }
            }
        }
        Err(e) => eprintln!("warning: failed to read wasi:config for OTel resource: {e:?}"),
    }

    otel::types::Resource {
        attributes: attrs.iter().map(|(k, v)| kv_str(k, v)).collect(),
        schema_url: None,
    }
}

pub(crate) fn scope() -> &'static otel::types::InstrumentationScope {
    SCOPE.get_or_init(|| otel::types::InstrumentationScope {
        name: PKG_NAME.into(),
        version: Some(PKG_VERSION.into()),
        schema_url: None,
        attributes: vec![],
    })
}

// --- Logs ---

/// Emit a trace-correlated INFO log via `wasi:otel/logs`.
pub(crate) fn otel_log(msg: &str, trace_id: &str, span_id: &str, flags: TraceFlags) {
    otel::logs::on_emit(&otel::logs::LogRecord {
        timestamp: Some(now()),
        observed_timestamp: None,
        severity_text: Some("INFO".into()),
        severity_number: Some(9),
        body: Some(json_str(msg)),
        attributes: None,
        event_name: None,
        resource: Some(resource().clone()),
        instrumentation_scope: Some(scope().clone()),
        trace_id: Some(trace_id.into()),
        span_id: Some(span_id.into()),
        trace_flags: Some(flags),
    });
}

// --- Span lifecycle ---

/// RAII handle for an in-flight OTel span. `on_start` fires in `start`;
/// `on_end` fires in `Drop`. If the holder is dropped without an explicit
/// `finish_ok` (e.g., a `?` early-returned past it), the span ends with
/// `Status::Error` so the trace exporter sees a complete, accurate picture
/// rather than a leaked, never-ended span.
pub(crate) struct ActiveSpan {
    span_ctx: Option<otel::tracing::SpanContext>,
    parent_span_id: String,
    kind: SpanKind,
    name: &'static str,
    start_time: wall_clock::Datetime,
    attributes: Vec<otel::types::KeyValue>,
    events: Vec<Event>,
    status: otel::tracing::Status,
}

impl ActiveSpan {
    pub(crate) fn start(
        trace_id: &str,
        flags: TraceFlags,
        parent_span_id: impl Into<String>,
        kind: SpanKind,
        name: &'static str,
    ) -> Self {
        let span_ctx = otel::tracing::SpanContext {
            trace_id: trace_id.into(),
            span_id: new_span_id(),
            trace_flags: flags,
            is_remote: false,
            trace_state: vec![],
        };
        otel::tracing::on_start(&span_ctx);
        Self {
            span_ctx: Some(span_ctx),
            parent_span_id: parent_span_id.into(),
            kind,
            name,
            start_time: now(),
            attributes: vec![],
            events: vec![],
            status: otel::tracing::Status::Unset,
        }
    }

    pub(crate) fn span_id(&self) -> &str {
        &self
            .span_ctx
            .as_ref()
            .expect("span_ctx is Some until Drop")
            .span_id
    }

    pub(crate) fn finish_ok(mut self, attributes: Vec<otel::types::KeyValue>) {
        self.attributes = attributes;
        self.status = otel::tracing::Status::Ok;
        // Drop runs `on_end` with the populated state.
    }

    pub(crate) fn finish_ok_with(
        mut self,
        attributes: Vec<otel::types::KeyValue>,
        events: Vec<Event>,
    ) {
        self.attributes = attributes;
        self.events = events;
        self.status = otel::tracing::Status::Ok;
    }
}

impl Drop for ActiveSpan {
    fn drop(&mut self) {
        let Some(span_ctx) = self.span_ctx.take() else {
            return;
        };
        let status = match std::mem::replace(&mut self.status, otel::tracing::Status::Unset) {
            otel::tracing::Status::Unset => otel::tracing::Status::Error(
                "span exited via early return without explicit completion".into(),
            ),
            other => other,
        };
        otel::tracing::on_end(&otel::tracing::SpanData {
            span_context: span_ctx,
            parent_span_id: std::mem::take(&mut self.parent_span_id),
            span_kind: self.kind,
            name: self.name.into(),
            start_time: self.start_time,
            end_time: now(),
            attributes: std::mem::take(&mut self.attributes),
            events: std::mem::take(&mut self.events),
            links: vec![],
            status,
            instrumentation_scope: scope().clone(),
            dropped_attributes: 0,
            dropped_events: 0,
            dropped_links: 0,
        });
    }
}

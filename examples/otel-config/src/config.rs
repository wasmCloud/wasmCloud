//! Per-request app config sourced from `wasi:config/store` (populated by
//! `workload.environment.{configFrom,secretFrom}` in `.wash/config.yaml`).

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use wstd::time::Duration;

use crate::bindings::wasi::config::store::get_all;
use crate::otel::{TraceFlags, otel_log};

const DEFAULT_OUTBOUND_HOST: &str = "example.com";
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 5_000;

/// OTLP endpoint to display in the demo UI. Matches the
/// `OTEL_EXPORTER_OTLP_ENDPOINT` value set by `.wash/config.yaml` and the
/// .NET Aspire dashboard's gRPC port. Guest components can't currently read
/// process env back out, so this is a static label rather than a live read.
const DISPLAY_OTLP_ENDPOINT: &str = "http://localhost:18889";

pub(crate) fn otlp_endpoint() -> &'static str {
    DISPLAY_OTLP_ENDPOINT
}

pub(crate) struct AppConfig {
    pub(crate) outbound_url: String,
    pub(crate) request_timeout: Duration,
    pub(crate) upstream_api_token: Option<String>,
}

static APP_CONFIG: OnceLock<AppConfig> = OnceLock::new();

pub(crate) fn app_config() -> &'static AppConfig {
    APP_CONFIG.get_or_init(build_app_config)
}

fn build_app_config() -> AppConfig {
    let mut entries: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(config) = get_all() {
        for (k, v) in config {
            entries.insert(k, v);
        }
    }
    // `OUTBOUND_URL`, when set, is used verbatim. This lets a workload point at
    // a sibling workload over `http://host:port/` (see the multi-workload
    // roll-up demo in the README), where the bare-host + implicit-`https`
    // shape of `OUTBOUND_HOST` doesn't fit. Otherwise fall back to
    // `https://{OUTBOUND_HOST}/`.
    let outbound_url = entries.get("OUTBOUND_URL").cloned().unwrap_or_else(|| {
        let outbound_host = entries
            .get("OUTBOUND_HOST")
            .map(String::as_str)
            .unwrap_or(DEFAULT_OUTBOUND_HOST);
        format!("https://{outbound_host}/")
    });
    let timeout_ms = entries
        .get("REQUEST_TIMEOUT_MS")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS);
    AppConfig {
        outbound_url,
        request_timeout: Duration::from_millis(timeout_ms),
        upstream_api_token: entries.get("UPSTREAM_API_TOKEN").cloned(),
    }
}

/// One-shot startup audit: emit a single OTel log line listing every
/// `wasi:config/store` key visible to the component. **Keys only**,
/// values are deliberately not included because secret values
/// (`workload.environment.secretFrom` entries) land in this same map.
///
/// Latch is set only on success, so a transient cold-start failure of
/// `wasi:config` does not permanently silence the audit log.
pub(crate) fn log_runtime_config(trace_id: &str, span_id: &str, flags: TraceFlags) {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if LOGGED.load(Ordering::Relaxed) {
        return;
    }
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
    LOGGED.store(true, Ordering::Relaxed);
}

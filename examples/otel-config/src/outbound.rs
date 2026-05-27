//! Outgoing HTTP request with W3C trace context propagation.

use anyhow::{Context, Result, bail};
use wstd::future::FutureExt;
use wstd::http::{Body, Client, HeaderValue, Request};

use crate::config::app_config;
use crate::otel::{TraceFlags, traceparent};

pub(crate) async fn make_outgoing_request(
    trace_id: &str,
    span_id: &str,
    flags: TraceFlags,
) -> Result<Vec<u8>> {
    let cfg = app_config();
    let mut client = Client::new();
    // Per-chunk timers catch a stalled connection mid-stream; the surrounding
    // `.timeout(cfg.request_timeout)` is the hard wall-clock budget so a
    // slowloris-style upstream that drips bytes within the per-chunk window
    // can't stretch the total call to `timeout * N`.
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

    let send_and_read = async {
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
    };

    send_and_read
        .timeout(cfg.request_timeout)
        .await
        .with_context(|| format!("outgoing request to {} timed out", cfg.outbound_url))?
}

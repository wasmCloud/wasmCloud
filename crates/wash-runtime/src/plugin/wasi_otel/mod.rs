//! # WASI OpenTelemetry Plugin
//! This module implements an OpenTelemetry plugin for the wasmCloud runtime,
//! providing the `wasi:otel@0.2.0-rc.2` interfaces.

mod convert;

pub use convert::otel_span_context_to_wit;
use convert::{
    convert_span_kind, convert_status, convert_wasi_log_record, extract_counter_values,
    extract_gauge_values, extract_span_attributes, extract_span_events, extract_span_links,
    summarize_resource_metrics, summarize_span_data, wasi_span_parent_context,
    wit_span_context_to_otel,
};

use anyhow::bail;
use opentelemetry::logs::{Logger, LoggerProvider};
use opentelemetry::trace::Span as _;

use opentelemetry::KeyValue;
use opentelemetry::trace::SpanContext;
use opentelemetry_sdk::logs::{BatchLogProcessor, SdkLoggerProvider};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter, WithExportConfig};

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::plugin::{HostPlugin, WitInterfaces, WorkloadItem, WorkloadTracker};
use crate::wit::{WitInterface, WitWorld};

const WASI_OTEL_ID: &str = "wasi-otel";

/// OTel gRPC default per the OTLP/gRPC spec.
const DEFAULT_OTLP_GRPC_ENDPOINT: &str = "http://localhost:4317";

/// OTel HTTP default per the OTLP/HTTP spec.
const DEFAULT_OTLP_HTTP_ENDPOINT: &str = "http://localhost:4318";

/// TODO: This message is temporary. His only purpose is to allow compilation and test while http
/// protocol not supported. Must be removed before merge on main branch.
const UNSUPPORTED_HTTP_MSG: &str = "OTLP protocol 'http/protobuf' is not yet supported; use 'grpc'";

mod bindings {
    wasmtime::component::bindgen!({
        world: "otel",
        imports: { default: async | trappable },
    });
}

use bindings::wasi::otel::tracing::{SpanContext as WitSpanContext, TraceFlags as WitTraceFlags};

/// OTLP wire protocol used to reach an [`OtelTarget`].
///
/// This selects the exporter *transport* config in builder (`.with_tonic()` vs `.with_http()`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum OtelProtocol {
    /// OTLP over gRPC.
    #[default]
    Grpc,
    /// OTLP over HTTP with protobuf-encoded bodies.
    /// Not yet implemented.
    HttpProtobuf,
}

impl std::fmt::Display for OtelProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Grpc => write!(f, "grpc"),
            Self::HttpProtobuf => write!(f, "http/protobuf"),
        }
    }
}

impl OtelProtocol {
    /// The OTLP spec's conventional default endpoint for this protocol.
    fn default_endpoint(self) -> String {
        match self {
            Self::Grpc => DEFAULT_OTLP_GRPC_ENDPOINT.to_string(),
            Self::HttpProtobuf => DEFAULT_OTLP_HTTP_ENDPOINT.to_string(),
        }
    }
}

/// A single OTLP destination.
///
/// Construct via [`OtelTarget::builder`].
#[derive(Clone, Debug, bon::Builder)]
#[non_exhaustive]
pub struct OtelTarget {
    /// OTLP endpoint, e.g. `http://localhost:4317`.
    #[builder(into)]
    pub endpoint: String,
    /// OTLP wire protocol for this target.
    pub protocol: Option<OtelProtocol>,
}

/// Configuration for the [`WasiOtel`] plugin.
///
/// Construct via [`WasiOtelConfig::builder`] (or [`Default::default`]) to
/// stay forward-compatible with new fields.
///
/// # Examples
///
/// ```
/// use wash_runtime::plugin::wasi_otel::WasiOtelConfig;
///
/// let cfg = WasiOtelConfig::builder()
///     .service_name("my-service")
///     .build();
/// assert_eq!(cfg.service_name, "my-service");
/// ```
#[derive(Clone, Debug, bon::Builder)]
#[non_exhaustive]
pub struct WasiOtelConfig {
    /// `service.name` resource attribute attached to all exported spans,
    /// metrics, and logs. Defaults to the plugin id (`wasi-otel`).
    #[builder(default = WASI_OTEL_ID.to_string(), into)]
    pub service_name: String,

    /// Host/platform telemetry target. When unset, falls back to
    /// `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` / `OTEL_EXPORTER_OTLP_ENDPOINT` /
    /// `OTEL_EXPORTER_OTLP_PROTOCOL`. Preserves prior behavior for deployments that only set the standard env vars.
    pub host: Option<OtelTarget>,

    /// Workload/application telemetry target, used for all spans, logs, and
    /// metrics emitted by components through `wasi:otel`. When unset, falls
    /// back to `host`.
    pub workload: Option<OtelTarget>,
}

impl Default for WasiOtelConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// A fully-resolved OTLP destination — a concrete endpoint and protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedTarget {
    endpoint: String,
    protocol: OtelProtocol,
}

#[derive(Clone, Debug, Default)]
struct EnvDefaults {
    /// From `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` / `OTEL_EXPORTER_OTLP_ENDPOINT`.
    endpoint: Option<String>,
    /// From `OTEL_EXPORTER_OTLP_PROTOCOL`.
    protocol: Option<OtelProtocol>,
}

impl WasiOtelConfig {
    /// Resolves the effective `(host, workload)` target pair.
    fn resolve_targets(&self, env: &EnvDefaults) -> (ResolvedTarget, ResolvedTarget) {
        let host_protocol = self
            .host
            .as_ref()
            .and_then(|t| t.protocol)
            .or(env.protocol)
            .unwrap_or_default();
        let host_endpoint = self
            .host
            .as_ref()
            .map(|t| t.endpoint.clone())
            .or_else(|| env.endpoint.clone())
            .unwrap_or_else(|| host_protocol.default_endpoint());
        let host = ResolvedTarget {
            endpoint: host_endpoint,
            protocol: host_protocol,
        };

        let workload_protocol = self
            .workload
            .as_ref()
            .and_then(|t| t.protocol)
            .unwrap_or(host.protocol);
        let workload_endpoint = self
            .workload
            .as_ref()
            .map(|t| t.endpoint.clone())
            .unwrap_or_else(|| host.endpoint.clone());
        let workload = ResolvedTarget {
            endpoint: workload_endpoint,
            protocol: workload_protocol,
        };

        (host, workload)
    }
}

/// Per-component context tracking
#[allow(dead_code)]
struct ComponentContext {
    component_id: String,
    workload_name: String,
    /// Current span context for this component's execution
    current_span_context: Option<SpanContext>,
}

/// WASI OpenTelemetry Plugin
pub struct WasiOtel {
    config: WasiOtelConfig,
    tracker: Arc<RwLock<WorkloadTracker<(), ComponentContext>>>,
    /// Meter provider for metrics export
    meter_provider: Arc<RwLock<Option<SdkMeterProvider>>>,
    tracer_provider: Arc<RwLock<Option<SdkTracerProvider>>>,
    logger_provider: Arc<RwLock<Option<SdkLoggerProvider>>>,
}

impl Default for WasiOtel {
    fn default() -> Self {
        Self::new(WasiOtelConfig::default())
    }
}

impl WasiOtel {
    /// Construct the plugin with the given configuration.
    pub fn new(config: WasiOtelConfig) -> Self {
        Self {
            config,
            tracker: Arc::new(RwLock::new(WorkloadTracker::default())),
            meter_provider: Arc::new(RwLock::new(None)),
            tracer_provider: Arc::new(RwLock::new(None)),
            logger_provider: Arc::new(RwLock::new(None)),
        }
    }
}

#[async_trait::async_trait]
impl HostPlugin for WasiOtel {
    fn id(&self) -> &'static str {
        WASI_OTEL_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasi:otel/types,tracing,metrics,logs@0.2.0-rc.2",
            )]),
            ..Default::default()
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        // Absent explicit config, fall back to the standard `OTEL_EXPORTER_OTLP_*`
        // env vars, then a protocol-dependent default — preserves prior behavior
        // for deployments that only set the standard env vars.
        let env_endpoint = std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
            .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT"))
            .ok();
        let env_protocol = match std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL") {
            Ok(v) if v == "grpc" => Some(OtelProtocol::Grpc),
            Ok(v) if v == "http/protobuf" => Some(OtelProtocol::HttpProtobuf),
            Ok(v) => {
                // An unparseable value shouldn't stop the host from booting;
                // fall through to the next link in the chain instead.
                tracing::warn!(
                    value = %v,
                    "Unrecognized OTEL_EXPORTER_OTLP_PROTOCOL value, ignoring"
                );
                None
            }
            Err(_) => None,
        };
        let (host, workload) = self.config.resolve_targets(&EnvDefaults {
            endpoint: env_endpoint,
            protocol: env_protocol,
        });

        tracing::info!(
            host_endpoint = %host.endpoint,
            host_protocol = %host.protocol,
            workload_endpoint = %workload.endpoint,
            workload_protocol = %workload.protocol,
            "Starting WASI OTel plugin"
        );

        // Every signal this plugin currently exports (span/log/metric handlers
        // below) is workload-originated, so all three exporters target
        // `workload`. `host` has no traffic of its own yet; it only matters as
        // the default `workload` falls back to.
        let span_exporter = build_span_exporter(&workload)?;
        let log_exporter = build_log_exporter(&workload)?;
        let metric_exporter = build_metric_exporter(&workload)?;

        // processor
        let processor = BatchLogProcessor::builder(log_exporter).build();

        // Initialize all providers
        let tracer_provider = opentelemetry_sdk::trace::TracerProviderBuilder::default()
            .with_batch_exporter(span_exporter)
            .with_resource(
                opentelemetry_sdk::Resource::builder_empty()
                    .with_attributes([KeyValue::new(
                        "service.name",
                        self.config.service_name.clone(),
                    )])
                    .build(),
            )
            .build();
        let logger_provider = opentelemetry_sdk::logs::LoggerProviderBuilder::default()
            .with_log_processor(processor)
            .with_resource(
                opentelemetry_sdk::Resource::builder_empty()
                    .with_attributes([KeyValue::new(
                        "service.name",
                        self.config.service_name.clone(),
                    )])
                    .build(),
            )
            .build();
        let meter_provider = SdkMeterProvider::builder()
            .with_periodic_exporter(metric_exporter)
            .with_resource(
                opentelemetry_sdk::Resource::builder_empty()
                    .with_attributes([KeyValue::new(
                        "service.name",
                        self.config.service_name.clone(),
                    )])
                    .build(),
            )
            .build();

        *self.tracer_provider.write().await = Some(tracer_provider);
        *self.logger_provider.write().await = Some(logger_provider);
        *self.meter_provider.write().await = Some(meter_provider);

        tracing::info!("WASI OTel plugin started");
        Ok(())
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        // Add all wasi:otel interfaces to linker
        bindings::wasi::otel::types::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;
        bindings::wasi::otel::tracing::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;
        bindings::wasi::otel::metrics::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;
        bindings::wasi::otel::logs::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;

        // Register component context for tracking
        let ctx = ComponentContext {
            component_id: component_handle.id().to_string(),
            workload_name: component_handle.workload_name().to_string(),
            current_span_context: None,
        };

        let WorkloadItem::Component(component_handle) = component_handle else {
            bail!("Service can not be tracked");
        };

        self.tracker
            .write()
            .await
            .add_component(component_handle, ctx);

        tracing::info!(
            component_id = component_handle.id(),
            "WASI OTel interfaces bound to component"
        );
        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        self.tracker
            .write()
            .await
            .remove_workload(workload_id)
            .await;
        tracing::info!(workload_id, "WASI OTel unbound from workload");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        tracing::info!("Stopping WASI OTel plugin");

        // Flush and shutdown all providers
        if let Some(provider) = self.tracer_provider.write().await.take() {
            let _ = provider.force_flush();
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.logger_provider.write().await.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.meter_provider.write().await.take() {
            let _ = provider.shutdown();
        }

        tracing::info!("WASI OTel plugin stopped");
        Ok(())
    }
}

/// NOTE: Could be impl in `ResolvedTarget` instead of root level functions
/// But the returned types SpanExporter, LogExporter, MetricExporter are not
/// related to ResolvedTarget.
fn build_span_exporter(target: &ResolvedTarget) -> anyhow::Result<SpanExporter> {
    match target.protocol {
        OtelProtocol::Grpc => SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&target.endpoint)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create span exporter: {e}")),
        OtelProtocol::HttpProtobuf => anyhow::bail!("{UNSUPPORTED_HTTP_MSG}"),
    }
}

fn build_log_exporter(target: &ResolvedTarget) -> anyhow::Result<LogExporter> {
    match target.protocol {
        OtelProtocol::Grpc => LogExporter::builder()
            .with_tonic()
            .with_endpoint(&target.endpoint)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create log exporter: {e}")),
        OtelProtocol::HttpProtobuf => anyhow::bail!("{UNSUPPORTED_HTTP_MSG}"),
    }
}

fn build_metric_exporter(target: &ResolvedTarget) -> anyhow::Result<MetricExporter> {
    match target.protocol {
        OtelProtocol::Grpc => MetricExporter::builder()
            .with_tonic()
            .with_endpoint(&target.endpoint)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create metric exporter: {e}")),
        OtelProtocol::HttpProtobuf => anyhow::bail!("{UNSUPPORTED_HTTP_MSG}"),
    }
}

// OTel Logs
impl<'a> bindings::wasi::otel::logs::Host for ActiveCtx<'a> {
    async fn on_emit(
        &mut self,
        data: bindings::wasi::otel::logs::LogRecord,
    ) -> wasmtime::Result<()> {
        tracing::info!(?data, "emitting log record");
        if let Ok(plugin) = self.ctx.try_get_plugin::<WasiOtel>(WASI_OTEL_ID) {
            let service_name = plugin.config.service_name.clone();
            let provider = plugin.logger_provider.read().await;

            if let Some(ref provider) = *provider {
                let logger = provider.logger(service_name.clone());
                let mut otel_record = logger.create_log_record();
                convert_wasi_log_record(data, &mut otel_record, service_name.clone());
                logger.emit(otel_record);
            }
        }
        Ok(())
    }
}

// OTel Metrics
impl<'a> bindings::wasi::otel::metrics::Host for ActiveCtx<'a> {
    async fn export(
        &mut self,
        resource_metrics: bindings::wasi::otel::metrics::ResourceMetrics,
    ) -> wasmtime::Result<Result<(), bindings::wasi::otel::metrics::Error>> {
        if let Ok(plugin) = self.ctx.try_get_plugin::<WasiOtel>(WASI_OTEL_ID) {
            // Summarize incoming metrics for logging
            let summary = summarize_resource_metrics(&resource_metrics);
            tracing::info!(
                total_scopes = summary.total_scopes,
                total_metrics = summary.total_metrics,
                metric_names = ?summary.metric_names,
                "Processing WASI resource metrics"
            );

            // Get the meter provider to record values
            let provider_guard = plugin.meter_provider.read().await;
            if let Some(ref provider) = *provider_guard {
                use opentelemetry::metrics::MeterProvider;
                let meter = provider.meter("wasi-otel");

                // Record gauge values
                for (name, value, attrs) in extract_gauge_values(&resource_metrics) {
                    let gauge = meter.f64_gauge(name).build();
                    let kv_attrs: Vec<KeyValue> = attrs
                        .into_iter()
                        .map(|(k, v)| KeyValue::new(k, v))
                        .collect();
                    gauge.record(value, &kv_attrs);
                }

                // Record counter values
                for (name, value, is_monotonic, attrs) in extract_counter_values(&resource_metrics)
                {
                    let kv_attrs: Vec<KeyValue> = attrs
                        .into_iter()
                        .map(|(k, v)| KeyValue::new(k, v))
                        .collect();
                    if is_monotonic {
                        let counter = meter.f64_counter(name).build();
                        counter.add(value, &kv_attrs);
                    } else {
                        let up_down = meter.f64_up_down_counter(name).build();
                        up_down.add(value, &kv_attrs);
                    }
                }

                // Force flush to export recorded metrics
                if let Err(e) = provider.force_flush() {
                    tracing::warn!(error = %e, "Failed to flush metrics");
                    return Ok(Err(format!("Failed to flush metrics: {e}")));
                }

                tracing::info!(
                    total_metrics = summary.total_metrics,
                    "Successfully processed WASI metrics"
                );
            } else {
                tracing::warn!("Meter provider not initialized");
                return Ok(Err("Meter provider not initialized".to_string()));
            }
        }

        Ok(Ok(()))
    }
}

// OTel Tracing
impl<'a> bindings::wasi::otel::tracing::Host for ActiveCtx<'a> {
    async fn on_start(
        &mut self,
        span_context: bindings::wasi::otel::tracing::SpanContext,
    ) -> wasmtime::Result<()> {
        // Log the span start - the actual span is managed by the guest
        tracing::info!(
            trace_id = %span_context.trace_id,
            span_id = %span_context.span_id,
            is_remote = span_context.is_remote,
            "WASI span started"
        );
        Ok(())
    }

    async fn on_end(
        &mut self,
        span_data: bindings::wasi::otel::tracing::SpanData,
    ) -> wasmtime::Result<()> {
        if let Ok(plugin) = self.ctx.try_get_plugin::<WasiOtel>(WASI_OTEL_ID) {
            let summary = summarize_span_data(&span_data);
            tracing::info!(
                name = %summary.name,
                trace_id = %summary.trace_id,
                span_id = %summary.span_id,
                parent_span_id = %summary.parent_span_id,
                kind = %summary.kind,
                status = %summary.status,
                attribute_count = summary.attribute_count,
                event_count = summary.event_count,
                link_count = summary.link_count,
                "Processing WASI span end"
            );

            let provider_guard = plugin.tracer_provider.read().await;
            if let Some(ref provider) = *provider_guard {
                use opentelemetry::trace::{SpanBuilder, Tracer, TracerProvider};

                let tracer = provider.tracer(plugin.config.service_name.clone());

                // Build a span with the data from WASI, preserving the guest's own
                // trace/span IDs and parent linkage instead of letting the SDK mint
                // fresh IDs and fall back to whatever host span is ambient.
                let wasi_span_context = wit_span_context_to_otel(&span_data.span_context);
                let parent_cx = wasi_span_parent_context(&span_data);
                let span_kind = convert_span_kind(span_data.span_kind);
                let status = convert_status(&span_data.status);
                let attributes = extract_span_attributes(&span_data);
                let events = extract_span_events(&span_data);
                let links = extract_span_links(&span_data);

                // Create a span builder with the WASI span data
                let mut builder = SpanBuilder::from_name(span_data.name.clone())
                    .with_trace_id(wasi_span_context.trace_id())
                    .with_span_id(wasi_span_context.span_id())
                    .with_kind(span_kind)
                    .with_attributes(attributes)
                    .with_links(links);

                // Set start time
                builder = builder.with_start_time(summary.start_time);

                // Start the span, parented according to the guest's own nesting
                let mut span = tracer.build_with_context(builder, &parent_cx);

                // Add events to the span
                for (event_name, _event_time, event_attrs) in events {
                    span.add_event(event_name, event_attrs);
                }

                // Set status
                span.set_status(status);

                // End the span with the end time from WASI
                span.end_with_timestamp(summary.end_time);

                tracing::info!(
                    name = %summary.name,
                    trace_id = %summary.trace_id,
                    "Successfully exported WASI span"
                );
            } else {
                tracing::warn!("Tracer provider not initialized");
            }
        }
        Ok(())
    }

    async fn outer_span_context(&mut self) -> wasmtime::Result<WitSpanContext> {
        // Try to get the current span context from the OpenTelemetry context
        use opentelemetry::trace::TraceContextExt;
        let current_context = opentelemetry::Context::current();
        let span_context = current_context.span().span_context().clone();

        if span_context.is_valid() {
            tracing::info!(
                trace_id = %format!("{:032x}", span_context.trace_id()),
                span_id = %format!("{:016x}", span_context.span_id()),
                "Returning outer span context"
            );
            Ok(otel_span_context_to_wit(&span_context))
        } else {
            tracing::info!("No valid outer span context available");
            Ok(WitSpanContext {
                trace_id: String::new(),
                span_id: String::new(),
                trace_flags: WitTraceFlags::empty(),
                is_remote: false,
                trace_state: vec![],
            })
        }
    }
}

impl<'a> bindings::wasi::otel::types::Host for ActiveCtx<'a> {}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(endpoint: Option<&str>, protocol: Option<OtelProtocol>) -> EnvDefaults {
        EnvDefaults {
            endpoint: endpoint.map(str::to_string),
            protocol,
        }
    }

    #[test]
    fn workload_falls_back_to_explicit_host() {
        let config = WasiOtelConfig::builder()
            .host(OtelTarget::builder().endpoint("http://host:4317").build())
            .build();
        let (host, workload) = config.resolve_targets(&env(Some("http://env-default:4317"), None));
        assert_eq!(host.endpoint, "http://host:4317");
        assert_eq!(workload.endpoint, "http://host:4317");
        assert_eq!(host.protocol, OtelProtocol::Grpc);
        assert_eq!(workload.protocol, OtelProtocol::Grpc);
    }

    #[test]
    fn workload_falls_back_to_env_default_host() {
        let config = WasiOtelConfig::builder().build();
        let (host, workload) = config.resolve_targets(&env(Some("http://env-default:4317"), None));
        assert_eq!(host.endpoint, "http://env-default:4317");
        assert_eq!(workload.endpoint, "http://env-default:4317");
    }

    #[test]
    fn split_endpoints_resolve_independently() {
        let config = WasiOtelConfig::builder()
            .host(
                OtelTarget::builder()
                    .endpoint("http://platform:4317")
                    .build(),
            )
            .workload(OtelTarget::builder().endpoint("http://app:4317").build())
            .build();
        let (host, workload) = config.resolve_targets(&env(Some("http://env-default:4317"), None));
        assert_eq!(host.endpoint, "http://platform:4317");
        assert_eq!(workload.endpoint, "http://app:4317");
    }

    #[test]
    fn workload_only_leaves_host_at_env_default() {
        let config = WasiOtelConfig::builder()
            .workload(OtelTarget::builder().endpoint("http://app:4317").build())
            .build();
        let (host, workload) = config.resolve_targets(&env(Some("http://env-default:4317"), None));
        assert_eq!(host.endpoint, "http://env-default:4317");
        assert_eq!(workload.endpoint, "http://app:4317");
    }

    #[test]
    fn protocol_inherits_workload_from_host() {
        let config = WasiOtelConfig::builder()
            .host(
                OtelTarget::builder()
                    .endpoint("http://platform:4318")
                    .protocol(OtelProtocol::HttpProtobuf)
                    .build(),
            )
            // Sets only an endpoint, so its protocol should inherit from `host`.
            .workload(OtelTarget::builder().endpoint("http://app:4317").build())
            .build();
        let (host, workload) = config.resolve_targets(&env(None, None));
        assert_eq!(host.protocol, OtelProtocol::HttpProtobuf);
        assert_eq!(workload.protocol, OtelProtocol::HttpProtobuf);
        // The endpoint itself is NOT inherited, since workload set its own.
        assert_eq!(workload.endpoint, "http://app:4317");
    }

    #[test]
    fn protocol_falls_back_to_env_then_default() {
        let config = WasiOtelConfig::builder().build();

        let (host, workload) = config.resolve_targets(&env(None, Some(OtelProtocol::HttpProtobuf)));
        assert_eq!(host.protocol, OtelProtocol::HttpProtobuf);
        assert_eq!(workload.protocol, OtelProtocol::HttpProtobuf);

        let (host, workload) = config.resolve_targets(&env(None, None));
        assert_eq!(host.protocol, OtelProtocol::Grpc);
        assert_eq!(workload.protocol, OtelProtocol::Grpc);
    }

    #[test]
    fn split_protocols_resolve_independently() {
        // This is the case the host/workload split exists for: two collectors
        // that don't even speak the same OTLP transport.
        let config = WasiOtelConfig::builder()
            .host(
                OtelTarget::builder()
                    .endpoint("http://platform:4317")
                    .protocol(OtelProtocol::Grpc)
                    .build(),
            )
            .workload(
                OtelTarget::builder()
                    .endpoint("http://app:4318")
                    .protocol(OtelProtocol::HttpProtobuf)
                    .build(),
            )
            .build();
        let (host, workload) = config.resolve_targets(&env(None, None));
        assert_eq!(host.protocol, OtelProtocol::Grpc);
        assert_eq!(workload.protocol, OtelProtocol::HttpProtobuf);
    }

    #[test]
    fn protocol_dependent_default_endpoint() {
        let config = WasiOtelConfig::builder().build();

        let (host, workload) = config.resolve_targets(&env(None, None));
        assert_eq!(host.endpoint, DEFAULT_OTLP_GRPC_ENDPOINT);
        assert_eq!(workload.endpoint, DEFAULT_OTLP_GRPC_ENDPOINT);

        let (host, workload) = config.resolve_targets(&env(None, Some(OtelProtocol::HttpProtobuf)));
        assert_eq!(host.endpoint, DEFAULT_OTLP_HTTP_ENDPOINT);
        assert_eq!(workload.endpoint, DEFAULT_OTLP_HTTP_ENDPOINT);
    }

    #[test]
    fn explicit_endpoint_wins_over_protocol_dependent_default() {
        // A non-default collector port is a legitimate choice; an explicit
        // endpoint is honored even when it doesn't match the protocol's
        // conventional port.
        let config = WasiOtelConfig::builder()
            .host(
                OtelTarget::builder()
                    .endpoint("http://collector:4317")
                    .protocol(OtelProtocol::HttpProtobuf)
                    .build(),
            )
            .build();
        let (host, _workload) = config.resolve_targets(&env(None, None));
        assert_eq!(host.endpoint, "http://collector:4317");
        assert_eq!(host.protocol, OtelProtocol::HttpProtobuf);
    }

    #[test]
    fn http_protobuf_exporter_construction_is_not_yet_supported() {
        let target = ResolvedTarget {
            endpoint: "http://localhost:4318".to_string(),
            protocol: OtelProtocol::HttpProtobuf,
        };
        for err in [
            build_span_exporter(&target).unwrap_err(),
            build_log_exporter(&target).unwrap_err(),
            build_metric_exporter(&target).unwrap_err(),
        ] {
            assert!(err.to_string().contains("not yet supported"));
        }
    }
}

//! # WASI OpenTelemetry Plugin
//! This module implements an OpenTelemetry plugin for the wasmCloud runtime,
//! providing the `wasi:otel@0.2.0-rc.1` interfaces.

mod convert;

pub use convert::otel_span_context_to_wit;
use convert::{
    convert_span_kind, convert_status, convert_wasi_log_record, extract_counter_values,
    extract_gauge_values, extract_span_attributes, extract_span_events, summarize_resource_metrics,
    summarize_span_data, wit_span_context_to_otel,
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

use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter};

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::plugin::{HostPlugin, WorkloadItem, WorkloadTracker};
use crate::wit::{WitInterface, WitWorld};

const WASI_OTEL_ID: &str = "wasi-otel";

mod bindings {
    wasmtime::component::bindgen!({
        world: "otel",
        imports: { default: async | trappable },
    });
}

use bindings::wasi::otel::tracing::{SpanContext as WitSpanContext, TraceFlags as WitTraceFlags};

/// Plugin configuration
#[derive(Clone, Debug)]
pub struct WasiOtelConfig {
    pub endpoint: String,
    pub protocol: String,
    pub service_name: String,
    pub propagate_context: bool,
    pub batch_timeout_ms: u64,
}

impl Default for WasiOtelConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            protocol: String::new(),
            service_name: "wasi-otel".to_string(),
            propagate_context: true,
            batch_timeout_ms: 5000,
        }
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
        Self {
            config: WasiOtelConfig::default(),
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
                "wasi:otel/types,tracing,metrics,logs@0.2.0-rc.1",
            )]),
            ..Default::default()
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        tracing::info!(
            endpoint = %self.config.endpoint,
            protocol = %self.config.protocol,
            "Starting WASI OTel plugin"
        );

        // TODO: Add configurable endpoints/protocols to use. This would be beneficial for when you want to have Host otel go to Platform engineering teams,
        // And Workload otel go to a different backend for application monitoring.

        // set up the grpc span exporter
        let span_exporter = SpanExporter::builder()
            .with_tonic()
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create span exporter: {e}"))?;

        // set up the grpc log exporter
        let log_exporter = LogExporter::builder()
            .with_tonic()
            //.with_endpoint("http://localhost:5318")
            //.with_protocol(opentelemetry_otlp::Protocol::Grpc)
            .build()?;

        // set up metric exporter
        let metric_exporter = MetricExporter::builder()
            .with_tonic()
            //.with_endpoint("http://localhost:5318")
            //.with_protocol(opentelemetry_otlp::Protocol::Grpc)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create metric exporter: {e}"))?;

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
        _interfaces: HashSet<WitInterface>,
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
        _interfaces: HashSet<WitInterface>,
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

// OTel Logs
impl<'a> bindings::wasi::otel::logs::Host for ActiveCtx<'a> {
    async fn on_emit(
        &mut self,
        data: bindings::wasi::otel::logs::LogRecord,
    ) -> wasmtime::Result<()> {
        tracing::info!(?data, "emitting log record");
        if let Some(plugin) = self.ctx.get_plugin::<WasiOtel>(WASI_OTEL_ID) {
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
        if let Some(plugin) = self.ctx.get_plugin::<WasiOtel>(WASI_OTEL_ID) {
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
                    return Ok(Err(format!("Failed to flush metrics: {}", e)));
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
        if let Some(plugin) = self.ctx.get_plugin::<WasiOtel>(WASI_OTEL_ID) {
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

                // Build a span with the data from WASI
                // Note: The SDK generates its own span/trace IDs. The WASI span context is logged
                // for correlation purposes but the SDK span will have different IDs.
                let _wasi_span_context = wit_span_context_to_otel(&span_data.span_context);
                let span_kind = convert_span_kind(span_data.span_kind);
                let status = convert_status(&span_data.status);
                let attributes = extract_span_attributes(&span_data);
                let events = extract_span_events(&span_data);

                // Create a span builder with the WASI span data
                let mut builder = SpanBuilder::from_name(span_data.name.clone())
                    .with_kind(span_kind)
                    .with_attributes(attributes);

                // Set start time
                builder = builder.with_start_time(summary.start_time);

                // Start the span
                let mut span = tracer.build(builder);

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

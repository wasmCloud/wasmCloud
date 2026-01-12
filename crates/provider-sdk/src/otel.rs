/// Instrument a given `Context`, injecting current `tracing`-generated metadata
/// if one isn't present.
///
/// This functionality is exposed as a macro since the context for trace injection
/// should be at the *call site* of this macro (ex. inside some method annotated with `#[instrument]`)
///
/// This macro requires `provider_sdk` and `wasmcloud_tracing` to be imported
#[macro_export]
macro_rules! propagate_trace_for_ctx {
    ($ctx:ident) => {{
        use $crate::wasmcloud_tracing::context::{attach_span_context, TraceContextInjector};
        let trace_ctx = match $ctx {
            Some(ref ctx) if !ctx.tracing.is_empty() => ctx
                .tracing
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Vec<(String, String)>>(),

            _ => TraceContextInjector::default_with_span()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        attach_span_context(&trace_ctx);
    }};
}

/// Initialize observability for a given provider with host-supplied data, via [`tracing`].
///
/// This functionality exists as a macro due to the requirement that `tracing` be initialized
/// from *binary* code, rather than library code.
///
/// This macro loads host data and uses the provider-sdk to build a `tracing::Dispatch` and
/// relevant guards/internal structures to configure it with information relevant to the host
///
/// This macro introduces the following variables into scope:
/// - `__observability__guard`
///
/// # Arguments
/// * `provider_name` - An expression that evaluates to a `&str` which is the name of your provider
/// * `maybe_flamegraphs_path` - An expression that evaluates to a `Option<impl AsRef<Path>>` for flamegraph path
#[macro_export]
macro_rules! initialize_observability {
    ($provider_name:expr, $maybe_flamegraphs_path:expr) => {
        let __observability_guard = {
            use $crate::anyhow::Context as _;
            use $crate::tracing_subscriber::util::SubscriberInitExt as _;
            let $crate::HostData {
                config,
                otel_config,
                structured_logging,
                log_level,
                ..
            } = $crate::provider::load_host_data().context("failed to load host data")?;

            // Update OTEL configuration with overrides if provided via config to the provider
            let mut otel_config = otel_config.clone();
            for (k, v) in config.iter() {
                match k.to_uppercase().as_str() {
                    "OTEL_EXPORTER_OTLP_ENDPOINT" => {
                        otel_config.observability_endpoint = Some(v.clone())
                    }
                    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT" => {
                        otel_config.traces_endpoint = Some(v.clone())
                    }
                    "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT" => {
                        otel_config.metrics_endpoint = Some(v.clone())
                    }
                    "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT" => {
                        otel_config.logs_endpoint = Some(v.clone())
                    }
                    "OTEL_TRACES_SAMPLER" => {
                        otel_config.traces_sampler = Some(v.clone())
                    }
                    "OTEL_TRACES_SAMPLER_ARG" => {
                        otel_config.traces_sampler_arg = Some(v.clone())
                    }
                    "OTEL_BSP_MAX_CONCURRENT_EXPORTS" => {
                        let parsed = match v.parse::<usize>() {
                            Ok(v) => v,
                            Err(_) => {
                                eprintln!(
                                    "Failed to parse OTEL_BSP_MAX_CONCURRENT_EXPORTS as usize, using previously set value or default"
                                );
                                continue
                            }
                        };
                        otel_config.concurrent_exports = Some(parsed)
                    }
                    "OTEL_BSP_MAX_QUEUE_SIZE" => {
                        let parsed = match v.parse::<usize>() {
                            Ok(v) => v,
                            Err(_) => {
                                eprintln!(
                                    "Failed to parse OTEL_BSP_MAX_QUEUE_SIZE as usize, using previously set value or default"
                                );
                                continue
                            }
                        };
                        otel_config.max_batch_queue_size = Some(parsed)
                    }
                    _ => {}
                }
            }

            // Init logging
            //
            // NOTE: this *must* be done on the provider binary side, to avoid
            // colliding with the in-process observability setup that happens in the host.
            let (dispatch, _guard) = $crate::wasmcloud_tracing::configure_observability(
                $provider_name,
                &otel_config,
                *structured_logging,
                $maybe_flamegraphs_path,
                log_level.as_ref(),
                Some(&otel_config.trace_level),
            )
            .context("failed to configure observability")?;
            dispatch
                .try_init()
                .context("failed to initialize observability")?;
            _guard
        };
    };
}

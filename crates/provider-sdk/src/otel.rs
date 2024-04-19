/// Instrument a given [`provider_sdk::Context`], injecting current `tracing`-generated metadata
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

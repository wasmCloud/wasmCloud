//! Trace and span IDs for guest spans re-emitted through the host SDK.

use std::cell::Cell;

use opentelemetry::trace::{SpanContext, SpanId, TraceId};
use opentelemetry_sdk::trace::{IdGenerator, RandomIdGenerator};

thread_local! {
    static GUEST_IDS: Cell<Option<(TraceId, SpanId)>> = const { Cell::new(None) };
}

/// [`IdGenerator`] that hands the SDK the IDs a guest already assigned to a
/// span, so the exported span keeps them. Outside [`with_guest_ids`] it
/// generates random IDs.
#[derive(Debug, Default)]
pub(super) struct GuestIdGenerator(RandomIdGenerator);

impl IdGenerator for GuestIdGenerator {
    fn new_trace_id(&self) -> TraceId {
        match GUEST_IDS.get() {
            Some((trace_id, _)) => trace_id,
            None => self.0.new_trace_id(),
        }
    }

    fn new_span_id(&self) -> SpanId {
        match GUEST_IDS.get() {
            Some((_, span_id)) => span_id,
            None => self.0.new_span_id(),
        }
    }
}

/// Run `f` with [`GuestIdGenerator`] returning `ctx`'s trace and span IDs on
/// this thread. `f` must start the span synchronously.
pub(super) fn with_guest_ids<R>(ctx: &SpanContext, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<(TraceId, SpanId)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            GUEST_IDS.set(self.0);
        }
    }

    let _restore = Restore(GUEST_IDS.replace(Some((ctx.trace_id(), ctx.span_id()))));
    f()
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::Context;
    use opentelemetry::trace::{
        Span as _, SpanBuilder, TraceContextExt as _, TraceFlags, TraceState, Tracer as _,
        TracerProvider as _,
    };
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

    fn guest_context(trace_id: u128, span_id: u64) -> SpanContext {
        SpanContext::new(
            TraceId::from(trace_id),
            SpanId::from(span_id),
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        )
    }

    #[test]
    fn guest_ids_survive_export() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .with_id_generator(GuestIdGenerator::default())
            .build();
        let tracer = provider.tracer("test");

        let root = guest_context(0x1234, 0xabcd);
        with_guest_ids(&root, || {
            tracer.build_with_context(SpanBuilder::from_name("root"), &Context::new())
        })
        .end();

        let child = guest_context(0x1234, 0xef01);
        let parent_cx = Context::new().with_remote_span_context(root.clone());
        with_guest_ids(&child, || {
            tracer.build_with_context(SpanBuilder::from_name("child"), &parent_cx)
        })
        .end();

        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].span_context.trace_id(), root.trace_id());
        assert_eq!(spans[0].span_context.span_id(), root.span_id());
        assert_eq!(spans[1].span_context.trace_id(), root.trace_id());
        assert_eq!(spans[1].span_context.span_id(), child.span_id());
        assert_eq!(spans[1].parent_span_id, root.span_id());
        assert_eq!(GUEST_IDS.get(), None);
    }
}

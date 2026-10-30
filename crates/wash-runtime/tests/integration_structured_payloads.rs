//! A component plugin relays records and a result-valued future across stores.

#![cfg(feature = "host-component-plugins")]

use anyhow::Result;
use std::{
    collections::HashMap,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use wash_runtime::{
    engine::{Engine, WasmProposal, ctx::SharedCtx, workload::WorkloadItem},
    plugin::{HostPlugin, WitInterfaces, component_host::ComponentHostPlugin},
    types::{Component, Workload},
    wit::WitWorld,
};
use wasmtime::{
    AsContextMut, StoreContextMut,
    component::{
        ComponentType, Destination, FutureConsumer, FutureReader, Lift, Lower, Source,
        StreamConsumer, StreamProducer, StreamReader, StreamResult, Type, Val, VecBuffer,
        types::ComponentItem,
    },
};

#[derive(Debug, Clone, PartialEq, ComponentType, Lift, Lower)]
#[component(record)]
struct Entry {
    text: String,
    data: Vec<u8>,
    maybe: Option<u64>,
}

fn entry() -> Entry {
    Entry {
        text: "hello 🦀".into(),
        data: vec![0, 1, 255],
        maybe: Some(42),
    }
}

struct Native;
#[async_trait::async_trait]
impl HostPlugin for Native {
    fn id(&self) -> &'static str {
        "structured-native"
    }
    fn world(&self) -> WitWorld {
        WitWorld {
            imports: ["test:payload/native,types".into()].into(),
            ..Default::default()
        }
    }
    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> Result<()> {
        item.linker()?.instance("test:payload/types")?;
        if !interfaces.contains("test", "payload", &["native"]) {
            return Ok(());
        }
        item.linker()?
            .instance("test:payload/native")?
            .func_new_concurrent("relay", |accessor, _, params, results| {
                Box::pin(async move {
                    let future = accessor.with(|mut access| {
                        FutureReader::new(&mut access, async {
                            Ok::<_, wasmtime::Error>(Ok::<_, String>(entry()))
                        })?
                        .try_into_future_any(&mut access)
                    })?;
                    let [input] = params else {
                        wasmtime::bail!("expected one input stream");
                    };
                    let [result] = results else {
                        wasmtime::bail!("expected one result tuple");
                    };
                    *result = Val::Tuple(vec![input.clone(), Val::Future(future)]);
                    Ok(())
                })
            })?;
        Ok(())
    }
}

struct Collect(tokio::sync::mpsc::UnboundedSender<Entry>);

// The producer cannot finish until the consumer sees the first record.
// Buffering a complete stream anywhere along the round trip would deadlock.
struct Gated {
    first: bool,
    next: tokio::sync::oneshot::Receiver<()>,
}

impl StreamProducer<SharedCtx> for Gated {
    type Item = Entry;
    type Buffer = VecBuffer<Entry>;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _: StoreContextMut<'a, SharedCtx>,
        mut dst: Destination<'a, Entry, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        if finish {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        if this.first {
            this.first = false;
            dst.set_buffer(vec![entry()].into());
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        match std::future::Future::poll(Pin::new(&mut this.next), cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(_) => {
                dst.set_buffer(vec![entry(); 31].into());
                Poll::Ready(Ok(StreamResult::Dropped))
            }
        }
    }
}

impl StreamConsumer<SharedCtx> for Collect {
    type Item = Entry;
    fn poll_consume(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        mut store: StoreContextMut<SharedCtx>,
        mut source: Source<'_, Entry>,
        _: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut items = Vec::with_capacity(source.remaining(&mut store));
        source.read(&mut store, &mut items)?;
        for item in items {
            if self.0.send(item).is_err() {
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
        }
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

struct Outcome(Option<tokio::sync::oneshot::Sender<Result<Entry, String>>>);
impl FutureConsumer<SharedCtx> for Outcome {
    type Item = Result<Entry, String>;
    fn poll_consume(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        mut store: StoreContextMut<SharedCtx>,
        mut source: Source<'_, Self::Item>,
        _: bool,
    ) -> Poll<wasmtime::Result<()>> {
        let mut values = Vec::with_capacity(1);
        source.read(&mut store, &mut values)?;
        if let Some(value) = values.pop()
            && let Some(tx) = self.get_mut().0.take()
        {
            let _ = tx.send(value);
        }
        Poll::Ready(Ok(()))
    }
}

#[derive(Clone, Copy)]
enum Registration {
    Valid,
    Missing,
    Mismatched,
}

#[tokio::test]
async fn structured_stream_and_future_cross_component_plugin_stores() -> Result<()> {
    exercise(Registration::Valid).await
}

#[tokio::test]
async fn missing_payload_adapter_is_rejected() -> Result<()> {
    exercise(Registration::Missing).await
}

#[tokio::test]
async fn mismatched_payload_adapter_is_rejected() -> Result<()> {
    exercise(Registration::Mismatched).await
}

async fn exercise(registration: Registration) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(45), async {
        let engine = Engine::builder()
            .with_pooling_allocator(false)
            .with_wasm_proposal(WasmProposal::ComponentModelAsync)
            .build()?;
        let wasm = include_bytes!("wasm/structured_plugin.wasm").to_vec();
        let compiled = wasmtime::component::Component::new(engine.inner(), &wasm)?;
        let Some((ComponentItem::ComponentInstance(api), _)) =
            compiled.get_export(None, "test:payload/provider")
        else {
            anyhow::bail!("missing provider");
        };
        let func = api
            .exports(engine.inner())
            .find_map(|(name, item)| match item.ty {
                ComponentItem::ComponentFunc(f) if name == "relay" => Some(f),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("missing relay"))?;
        let Some((_, Type::Stream(stream))) = func.params().next() else {
            anyhow::bail!("missing stream");
        };
        let payload = stream
            .ty()
            .ok_or_else(|| anyhow::anyhow!("missing payload"))?;
        assert!(
            engine
                .stream_payloads()
                .register::<Entry>(Type::Stream(stream.clone()))
                .is_err()
        );
        match registration {
            Registration::Valid => engine.stream_payloads().register::<Entry>(payload)?,
            Registration::Missing => {}
            Registration::Mismatched => engine.stream_payloads().register::<String>(payload)?,
        }
        let Some(Type::Tuple(reply)) = func.results().next() else {
            anyhow::bail!("missing reply");
        };
        let Some(Type::Future(future)) = reply.types().nth(1) else {
            anyhow::bail!("missing future");
        };
        engine.stream_payloads().register::<Result<Entry, String>>(
            future
                .ty()
                .ok_or_else(|| anyhow::anyhow!("missing outcome"))?,
        )?;

        let native: Arc<dyn HostPlugin> = Arc::new(Native);
        let plugin = Arc::new(
            ComponentHostPlugin::builder()
                .id("structured-plugin")
                .wasm(&wasm)
                .engine(engine.clone())
                .native_plugins(HashMap::from([(native.id(), native.clone())]))
                .build()
                .await?,
        );
        plugin.start().await?;
        let plugin_trait: Arc<dyn HostPlugin> = plugin.clone();
        let resolved = engine
            .initialize_workload(
                "structured",
                Workload {
                    name: "structured".into(),
                    namespace: "test".into(),
                    annotations: HashMap::new(),
                    service: None,
                    volumes: vec![],
                    host_interfaces: vec![
                        "test:payload/provider".into(),
                        "test:payload/types".into(),
                    ],
                    components: vec![Component {
                        name: "caller".into(),
                        bytes: bytes::Bytes::from_static(include_bytes!(
                            "wasm/structured_caller.wasm"
                        )),
                        ..Default::default()
                    }],
                },
            )?
            .resolve(
                Some(&HashMap::from([
                    (plugin_trait.id(), plugin_trait),
                    (native.id(), native),
                ])),
                &Default::default(),
                &wash_runtime::host::HostRef::detached(),
                &Default::default(),
            )
            .await?;
        let id = resolved
            .components()
            .read_owned()
            .await
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing caller"))?;
        let mut store = resolved.new_store(&id).await?;
        let pre = resolved.instantiate_pre(&id).await?;
        let instance = pre.instantiate_async(&mut store).await?;
        let (_, api) = instance
            .get_export(&mut store, None, "test:payload/caller")
            .ok_or_else(|| anyhow::anyhow!("missing caller interface"))?;
        let (_, index) = instance
            .get_export(&mut store, Some(&api), "relay")
            .ok_or_else(|| anyhow::anyhow!("missing relay export"))?;
        let function = instance
            .get_func(&mut store, index)
            .ok_or_else(|| anyhow::anyhow!("missing relay function"))?;
        let (advance, next) = tokio::sync::oneshot::channel();
        let input = StreamReader::new(&mut store, Gated { first: true, next })?
            .try_into_stream_any(&mut store)?;
        let result = store
            .run_concurrent(async |accessor| {
                let mut results = [Val::Bool(false)];
                function
                    .call_concurrent(accessor, &[Val::Stream(input)], &mut results)
                    .await?;
                let Val::Tuple(mut reply) = results[0].clone() else {
                    wasmtime::bail!("not a tuple");
                };
                let Some(Val::Future(future)) = reply.pop() else {
                    wasmtime::bail!("not a future");
                };
                let Some(Val::Stream(stream)) = reply.pop() else {
                    wasmtime::bail!("not a stream");
                };
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
                accessor.with(|mut access| {
                    StreamReader::<Entry>::try_from_stream_any(stream)?
                        .pipe(access.as_context_mut(), Collect(tx))?;
                    FutureReader::<Result<Entry, String>>::try_from_future_any(future)?
                        .pipe(access.as_context_mut(), Outcome(Some(outcome_tx)))
                })?;
                let first = rx
                    .recv()
                    .await
                    .ok_or_else(|| wasmtime::format_err!("stream ended before its first record"))?;
                assert_eq!(first, entry());
                let _ = advance.send(());
                let mut items = vec![first];
                while let Some(item) = rx.recv().await {
                    items.push(item);
                }
                assert_eq!(items, vec![entry(); 32]);
                assert_eq!(
                    outcome_rx.await.map_err(|e| wasmtime::format_err!("{e}"))?,
                    Ok(entry())
                );
                wasmtime::Result::Ok(())
            })
            .await
            .and_then(|result| result);
        resolved.unbind_all_plugins().await?;
        plugin.stop().await?;
        match registration {
            Registration::Valid => result?,
            Registration::Missing => {
                let Err(error) = result else {
                    anyhow::bail!("unregistered payload crossed stores");
                };
                assert!(format!("{error:#}").contains("unregistered stream payload"));
            }
            Registration::Mismatched => {
                let Err(error) = result else {
                    anyhow::bail!("mismatched Rust payload type crossed stores");
                };
                // Wasmtime's payload typecheck rejected the transfer, rather than
                // the unregistered-payload path or anything incidental.
                let message = format!("{error:#}");
                assert!(
                    message.contains("expected `string`"),
                    "unexpected failure: {message}"
                );
            }
        }
        Result::<()>::Ok(())
    })
    .await??;
    Ok(())
}

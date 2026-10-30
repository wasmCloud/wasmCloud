use super::super::multiplexed::InMemoryBackend;
use super::*;
use bindings::{
    named_imports::wasmcloud::blobstore::blobstore::HostWithStore as Named,
    wasmcloud::blobstore::{
        blobstore::HostWithStore as Plain, container::HostContainerWithStore as Container,
    },
};
use std::collections::HashMap;

fn borrow<T: 'static>(resource: &Resource<T>) -> Resource<T> {
    Resource::new_borrow(resource.rep())
}
fn object(container: &str, name: &str) -> ObjectId {
    ObjectId {
        container: container.into(),
        object: name.into(),
    }
}

#[tokio::test]
async fn async_routes_keep_resources_and_streams_on_the_selected_backend() {
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    for named in [false, true] {
        let backend: BlobId = Arc::new(InMemoryBackend::new());
        let plugin = Arc::new(MultiplexedAsyncBlobstore::new());
        plugin
            .mux
            .set_default("workload", &HashMap::from([("".into(), backend.clone())]));
        let host_plugin: Arc<dyn HostPlugin + Send + Sync> = plugin;
        let ctx = crate::engine::ctx::Ctx::builder("workload", "component")
            .with_plugins(HashMap::from([(
                MULTIPLEXED_ASYNC_BLOBSTORE_ID,
                host_plugin,
            )]))
            .build();
        let mut config = wasmtime::Config::new();
        config
            .wasm_component_model_async(true)
            .concurrency_support(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let mut store = wasmtime::Store::new(&engine, SharedCtx::new(ctx));
        tokio::time::timeout(std::time::Duration::from_secs(10), store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let a = &host_accessor;
            macro_rules! route {
                ($method:ident $(, $arg:expr)*) => {
                    if named { <SharedCtx as Named<_>>::$method(a, backend.clone() $(, $arg)*).await.unwrap() }
                    else { <SharedCtx as Plain<_>>::$method(a $(, $arg)*).await.unwrap() }
                };
            }
            assert!(!route!(container_exists, "source".into()).unwrap());
            assert!(route!(get_container, "source".into()).is_err());
            let container = route!(create_container, "source".into()).unwrap();
            assert!(route!(create_container, "source".into()).is_err());
            assert!(route!(container_exists, "source".into()).unwrap());
            let reopened = route!(get_container, "source".into()).unwrap();
            a.with(|mut access| { access.get().table.delete(reopened).unwrap(); });
            assert_eq!(<SharedCtx as Container<_>>::name(a, borrow(&container)).await.unwrap().unwrap(), "source");
            assert_eq!(<SharedCtx as Container<_>>::info(a, borrow(&container)).await.unwrap().unwrap().name, "source");
            let body = a.with(|mut access| StreamReader::new(&mut access, b"abcdef".to_vec())).unwrap();
            <SharedCtx as Container<_>>::write_data(a, borrow(&container), "data".into(), body).await.unwrap().unwrap();
            assert_eq!(backend.get_data("source", "data", 0, u64::MAX).await.unwrap(), b"abcdef");
            let body = <SharedCtx as Container<_>>::get_data(a, borrow(&container), "data".into(), 1, 3).await.unwrap().unwrap();
            assert_eq!(crate::plugin::stream_collect::collect_stream(a, body, 10).await.unwrap().unwrap(), b"bcd");
            assert_eq!(<SharedCtx as Container<_>>::object_info(a, borrow(&container), "data".into()).await.unwrap().unwrap().size, 6);
            let destination = route!(create_container, "destination".into()).unwrap();
            route!(copy_object, object("source", "data"), object("destination", "copy")).unwrap();
            route!(move_object, object("destination", "copy"), object("destination", "moved")).unwrap();
            assert!(<SharedCtx as Container<_>>::has_object(a, borrow(&destination), "moved".into()).await.unwrap().unwrap());
            <SharedCtx as Container<_>>::delete_object(a, borrow(&destination), "moved".into()).await.unwrap().unwrap();
            backend.write_data("destination", "other", vec![1]).await.unwrap();
            <SharedCtx as Container<_>>::delete_objects(a, borrow(&destination), vec!["other".into()]).await.unwrap().unwrap();
            <SharedCtx as Container<_>>::clear(a, borrow(&container)).await.unwrap().unwrap();
            assert!(!<SharedCtx as Container<_>>::has_object(a, borrow(&container), "data".into()).await.unwrap().unwrap());
            route!(delete_container, "source".into()).unwrap();
            assert!(<SharedCtx as Container<_>>::info(a, borrow(&container)).await.unwrap().is_err());
            assert!(<SharedCtx as Container<_>>::get_data(a, borrow(&container), "data".into(), 0, 1).await.unwrap().is_err());
            assert!(<SharedCtx as Container<_>>::list_objects(a, borrow(&container)).await.unwrap().is_err());
            assert!(<SharedCtx as Container<_>>::object_info(a, borrow(&container), "data".into()).await.unwrap().is_err());
            assert!(<SharedCtx as Container<_>>::clear(a, borrow(&container)).await.unwrap().is_err());
            assert!(route!(copy_object, object("source", "missing"), object("destination", "copy")).is_err());
            assert!(route!(move_object, object("source", "missing"), object("destination", "copy")).is_err());
            a.with(|mut access| { access.get().table.delete(container).unwrap(); access.get().table.delete(destination).unwrap(); });
            Ok::<_, wasmtime::Error>(())
        })).await.unwrap().unwrap().unwrap();
    }
}

#[tokio::test]
async fn plain_calls_without_a_default_report_configuration_errors() {
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    let plugin: Arc<dyn HostPlugin + Send + Sync> = Arc::new(MultiplexedAsyncBlobstore::new());
    let ctx = crate::engine::ctx::Ctx::builder("workload", "component")
        .with_plugins(HashMap::from([(MULTIPLEXED_ASYNC_BLOBSTORE_ID, plugin)]))
        .build();
    let mut config = wasmtime::Config::new();
    config
        .wasm_component_model_async(true)
        .concurrency_support(true);
    let engine = wasmtime::Engine::new(&config).unwrap();
    let mut store = wasmtime::Store::new(&engine, SharedCtx::new(ctx));
    store.run_concurrent(async |store_accessor| {
        let a = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
        macro_rules! missing {
            ($method:ident $(, $arg:expr)*) => {
                assert!(matches!(<SharedCtx as Plain<_>>::$method(&a $(, $arg)*).await.unwrap(), Err(AsyncError::Other(message)) if message.contains("no default")));
            };
        }
        missing!(create_container, "data".into());
        missing!(get_container, "data".into());
        missing!(delete_container, "data".into());
        missing!(container_exists, "data".into());
        missing!(copy_object, object("data", "a"), object("data", "b"));
        missing!(move_object, object("data", "a"), object("data", "b"));
        Ok::<_, wasmtime::Error>(())
    }).await.unwrap().unwrap();
}
